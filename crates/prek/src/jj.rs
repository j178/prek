use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tracing::instrument;

use crate::git::{FileEntry, FileMode, path_from_git_bytes};
use crate::process;
use crate::process::Cmd;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Command(#[from] process::Error),

    #[error("Failed to find Jujutsu (jj): {0}")]
    JjNotFound(#[from] which::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Path to the `jj` executable, resolved via `PATH`.
pub(crate) static JJ: LazyLock<Result<PathBuf, which::Error>> =
    LazyLock::new(|| which::which("jj"));

/// A Jujutsu workspace's backing Git store, resolved from `.jj` metadata.
///
/// prek drives Jujutsu workspaces through Git, so the backing store is what
/// `git::git_cmd` points Git at when the workspace has no `.git` of its own.
#[derive(Debug)]
pub(crate) struct GitStore {
    /// The Jujutsu workspace root, which prek treats as the repository boundary.
    pub(crate) workspace_root: PathBuf,
    /// The Git directory backing the workspace's `.jj` metadata.
    pub(crate) git_dir: PathBuf,
}

/// Walk up from `start` looking for a directory containing `.jj/`.
pub(crate) fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".jj").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Read a path that jj stored as raw bytes in its metadata.
///
/// jj writes these files without a line terminator, and a path may not be valid UTF-8 or
/// may end in whitespace, so the bytes are decoded as a path rather than as text.
fn read_metadata_path(path: &Path) -> Result<PathBuf, Error> {
    let bytes = fs_err::read(path)?;
    path_from_git_bytes(&bytes)
        .map_err(|err| Error::Io(std::io::Error::new(std::io::ErrorKind::InvalidData, err)))
}

fn resolve_repo_dir(workspace_root: &Path) -> Result<Option<PathBuf>, Error> {
    let repo_dir_candidate = workspace_root.join(".jj").join("repo");
    if repo_dir_candidate.is_file() {
        let path = read_metadata_path(&repo_dir_candidate)?;
        let repo_dir = if path.is_absolute() {
            path
        } else {
            workspace_root.join(".jj").join(path)
        };
        return Ok(Some(repo_dir));
    }
    if repo_dir_candidate.is_dir() {
        return Ok(Some(repo_dir_candidate));
    }
    Ok(None)
}

/// Resolve the backing Git directory for a Jujutsu workspace.
///
/// For a primary (colocated) workspace, `.jj/repo` is a directory.
/// For a secondary workspace (created with `jj workspace add`), `.jj/repo` is a file
/// containing the absolute path to the main repo's `.jj/repo` directory.
pub(crate) fn resolve_backing_git_dir(workspace_root: &Path) -> Result<Option<PathBuf>, Error> {
    let Some(repo_dir) = resolve_repo_dir(workspace_root)? else {
        return Ok(None);
    };

    let git_target_file = repo_dir.join("store").join("git_target");
    let git_dir = if git_target_file.try_exists()? {
        let git_target = read_metadata_path(&git_target_file)?;
        if git_target.is_absolute() {
            git_target
        } else {
            repo_dir.join("store").join(git_target)
        }
    } else {
        // Fall back to `.jj/repo/store/git` for `jj git init --no-colocate` repositories.
        let store_git = repo_dir.join("store").join("git");
        if store_git.is_dir() {
            store_git
        } else {
            // Not a Git-backed jj repo (e.g. the native backend), which prek cannot drive.
            // Treat it as "no backing Git dir".
            return Ok(None);
        }
    };

    // Check existence before canonicalizing: `canonicalize()` errors on a missing
    // path, but a dangling `git_target` should surface as `Ok(None)`, not an error.
    if !git_dir.try_exists()? {
        return Ok(None);
    }

    // Canonicalize to resolve any `..` components now that we know it exists.
    // `dunce` avoids Windows `\\?\` UNC paths, which git handles poorly.
    let git_dir = dunce::canonicalize(&git_dir)?;
    Ok(Some(git_dir))
}

/// Resolve the Jujutsu workspace containing `cwd` and its backing Git store.
///
/// Returns `None` outside a Jujutsu workspace, or when the workspace has no usable
/// backing Git store (a non-Git backend, or broken `.jj` metadata).
pub(crate) fn resolve_git_store(cwd: &Path) -> Result<Option<GitStore>, Error> {
    let Some(workspace_root) = find_workspace_root(cwd) else {
        return Ok(None);
    };
    let Some(git_dir) = resolve_backing_git_dir(&workspace_root)? else {
        return Ok(None);
    };
    // Match the canonical spelling used for the Git root so the two can be compared.
    let workspace_root = dunce::canonicalize(&workspace_root).unwrap_or(workspace_root);
    Ok(Some(GitStore {
        workspace_root,
        git_dir,
    }))
}

/// Create a new `Cmd` for running Jujutsu.
///
/// Every caller parses jj's output, so styling is disabled: with `ui.color = "always"`
/// and a configured color rule, jj would wrap template output in ANSI sequences that the
/// parsers would read as part of a path.
pub(crate) fn jj_cmd() -> Result<Cmd, Error> {
    let mut cmd = Cmd::new(JJ.as_ref().map_err(|&e| Error::JjNotFound(e))?);
    cmd.arg("--color=never");
    Ok(cmd)
}

/// Rebase a workspace-root-relative path from jj onto `cwd`, dropping paths outside it.
///
/// `jj file list` and `jj diff` templates report paths relative to the workspace root,
/// even when jj runs from a subdirectory, while the Git backend reports paths relative
/// to the directory it runs in. This makes the two agree.
fn rebase_to_cwd(root: &Path, cwd: &Path, path: PathBuf) -> Option<PathBuf> {
    let prefix = cwd.strip_prefix(root).ok()?;
    if prefix.as_os_str().is_empty() {
        return Some(path);
    }
    path.strip_prefix(prefix).ok().map(Path::to_path_buf)
}

/// Template that makes `jj file list` print one path per line regardless of the
/// user's `templates.file_list` configuration.
///
/// Records are NUL-terminated, since a newline is a valid character in a repository
/// path and would otherwise split one path into several records.
const FILE_LIST_TEMPLATE: &str = r#"path ++ "\0""#;

/// Template that prints only paths with unresolved conflicts, one NUL-terminated
/// record each.
///
/// `jj resolve --list` is not usable here: it renders a display table, and a path of
/// 35 characters or more is separated from its description by a single space, which
/// cannot be parsed back into a path.
const CONFLICT_TEMPLATE: &str = r#"if(conflict, path ++ "\0", "")"#;

/// Template for `jj diff`, emitting `<status> <source file type> <source executable>
/// <path>` per record.
///
/// `source` describes the path before the change: it is empty for added paths, and
/// otherwise carries the mode Git reports for deletions in `--raw`. A rename reports
/// only its destination as the path, so it emits the removed source path as an extra
/// deletion record, matching Git's `--no-renames` split. Records are NUL-terminated
/// for the same reason as `FILE_LIST_TEMPLATE`.
///
/// `path` is workspace-root-relative (a `RepoPath`), regardless of the command's
/// working directory.
const DIFF_TEMPLATE: &str = r#"status_char ++ " " ++ source.file_type() ++ " " ++ source.executable() ++ " " ++ path ++ "\0" ++ if(status_char == "R", "D " ++ source.file_type() ++ " " ++ source.executable() ++ " " ++ source.path() ++ "\0", "")"#;

/// Iterate the NUL-terminated records of jj output.
fn records(output: &[u8]) -> impl Iterator<Item = &[u8]> {
    output
        .split(|&byte| byte == b'\0')
        .filter(|record| !record.is_empty())
}

fn parse_path_records(output: &[u8]) -> Vec<PathBuf> {
    records(output)
        .filter_map(|record| path_from_git_bytes(record).ok())
        .collect()
}

/// One entry from a `jj diff` rendered with `DIFF_TEMPLATE`.
struct DiffEntry {
    status: u8,
    source_type: Vec<u8>,
    source_executable: bool,
    path: PathBuf,
}

fn parse_diff_entries(output: &[u8]) -> Vec<DiffEntry> {
    records(output)
        .filter_map(|record| {
            let mut fields = record.splitn(4, |&byte| byte == b' ');
            let status = *fields.next()?.first()?;
            let source_type = fields.next()?.to_vec();
            let source_executable = fields.next()? == b"true";
            let path = path_from_git_bytes(fields.next()?).ok()?;
            Some(DiffEntry {
                status,
                source_type,
                source_executable,
                path,
            })
        })
        .collect()
}

/// Map a jj path kind before the change to the Git file mode reported for a deletion.
fn deleted_file_mode(source_type: &[u8], executable: bool) -> Option<FileMode> {
    match source_type {
        b"file" if executable => Some(FileMode::Executable),
        b"file" => Some(FileMode::Regular),
        b"symlink" => Some(FileMode::Symlink),
        b"git-submodule" => Some(FileMode::Submodule),
        _ => None,
    }
}

/// Convert `jj diff` output into file entries.
///
/// Deletions are dropped unless `include_deleted` is set, matching the Git backend,
/// whose `--diff-filter` default never reports deleted paths (running hooks on
/// nonexistent files is pointless).
fn diff_file_entries(output: &[u8], include_deleted: bool) -> Vec<FileEntry> {
    parse_diff_entries(output)
        .into_iter()
        .filter(|entry| include_deleted || entry.status != b'D')
        .map(|entry| FileEntry {
            deleted_mode: (entry.status == b'D')
                .then(|| deleted_file_mode(&entry.source_type, entry.source_executable))
                .flatten(),
            path: entry.path,
        })
        .collect()
}

/// Build a jj fileset expression matching `path` literally, relative to the
/// command's working directory. jj interprets bare path arguments as fileset
/// expressions, so metacharacters like `[` must be quoted; this mirrors git's
/// `--literal-pathspecs`.
/// Quote a path as a jj fileset string literal.
///
/// jj's string literals only recognize `\\` and `\"`, and treat everything else as
/// literal text. Rust's `{:?}` would escape more than that (`\u{200b}` and friends),
/// which jj rejects.
fn quote_fileset(path: &str) -> String {
    let escaped = path.replace('\\', "\\\\").replace('"', "\\\"");
    format!("cwd:\"{escaped}\"")
}

/// Build a jj fileset expression matching `path` literally, relative to the command's
/// working directory.
///
/// jj interprets bare path arguments as fileset expressions, so metacharacters like `[`
/// must be quoted; this mirrors git's `--literal-pathspecs`. Returns `None` when the path
/// has no UTF-8 spelling, which a fileset (text) cannot express; the caller then queries
/// a wider scope and filters the decoded paths itself.
// Windows paths always have a UTF-8 (lossy) spelling, so the wrap is only needed on Unix.
#[cfg_attr(windows, expect(clippy::unnecessary_wraps))]
fn literal_fileset(path: &Path) -> Option<String> {
    // jj filesets use forward slashes, and on Windows a `Path` renders with backslashes
    // that have to become separators. On Unix a backslash is an ordinary filename
    // character, so it is left alone.
    #[cfg(windows)]
    let path = path.to_string_lossy().replace('\\', "/");
    #[cfg(not(windows))]
    let path = path.to_str()?.to_owned();

    Some(quote_fileset(&path))
}

/// Build a literal fileset that scopes a query to `path`, expressed relative to `cwd`.
///
/// Callers pass either a path relative to `cwd` (as Git pathspecs are) or a rooted one
/// below it. A rooted path that is not below `cwd`, and `cwd` itself, scope to the whole
/// tree, because fileset paths are cwd-relative and an absolute path would match nothing.
/// Returns `None` for a path without a UTF-8 spelling, like `literal_fileset`.
fn scope_fileset(cwd: &Path, path: &Path) -> Option<String> {
    let relative = match path {
        path if !path.has_root() => path,
        path => path
            .strip_prefix(cwd)
            .ok()
            .filter(|relative| !relative.as_os_str().is_empty())
            .unwrap_or(Path::new(".")),
    };
    literal_fileset(relative)
}

/// Fileset matching everything under the command's working directory.
const CWD_FILESET: &str = r#"cwd:".""#;

/// List tracked files under `cwd`, returned relative to `cwd` like `git::ls_files`.
///
/// `root` is the workspace root, which jj reports paths against.
#[instrument(level = "trace", skip(paths))]
pub(crate) async fn ls_files<P>(
    root: &Path,
    cwd: &Path,
    paths: impl IntoIterator<Item = P>,
) -> Result<Vec<PathBuf>, Error>
where
    P: AsRef<Path>,
{
    let mut cmd = jj_cmd()?;
    cmd.current_dir(cwd)
        .arg("file")
        .arg("list")
        // Pin the output format: `jj file list`'s default template is user-overridable
        // via `templates.file_list`, which would otherwise break path parsing.
        .arg("-T")
        .arg(FILE_LIST_TEMPLATE);
    for path in paths {
        // An unrepresentable scope widens the query to the command directory; the caller
        // filters the decoded paths itself.
        cmd.arg(scope_fileset(cwd, path.as_ref()).unwrap_or_else(|| CWD_FILESET.to_string()));
    }

    let output = cmd.check(true).output().await?;
    Ok(parse_path_records(&output.stdout)
        .into_iter()
        .filter_map(|path| rebase_to_cwd(root, cwd, path))
        .collect())
}

/// Files changed in the current working-copy revision, relative to the workspace root.
///
/// `scope` narrows the query to one project directory inside the workspace.
#[instrument(level = "trace")]
pub(crate) async fn get_changed_files(
    root: &Path,
    scope: Option<&Path>,
    include_deleted: bool,
) -> Result<Vec<FileEntry>, Error> {
    let mut cmd = jj_cmd()?;
    cmd.current_dir(root)
        .arg("diff")
        .arg("-r")
        .arg("@")
        .arg("-T")
        .arg(DIFF_TEMPLATE);
    if let Some(scope_path) = scope
        && let Some(fileset) = scope_fileset(root, scope_path)
    {
        cmd.arg(fileset);
    }
    let output = cmd.check(true).output().await?;
    Ok(diff_file_entries(&output.stdout, include_deleted))
}

/// Patch of the working copy under `scope`, against its parent commit.
///
/// Compared only with other samples of this query, never parsed, so only the shape of the
/// output has to stay stable; `--git` pins it against `ui.diff` settings.
///
/// Sampling through jj keeps such a comparison meaningful: jj snapshots before answering,
/// so the sample does not depend on where a snapshot happens to fall. A Git diff of the
/// same workspace would instead change whenever jj rewrote the index, which every jj
/// command does in a colocated workspace.
#[instrument(level = "trace")]
pub(crate) async fn diff_worktree(root: &Path, scope: Option<&Path>) -> Result<Vec<u8>, Error> {
    let mut cmd = jj_cmd()?;
    cmd.arg("diff").arg("--git").arg("-r").arg("@");
    match scope {
        Some(scope_path) => match scope_fileset(root, scope_path) {
            Some(fileset) => {
                cmd.current_dir(root).arg(fileset);
            }
            // jj cannot name a path outside its UTF-8 filesets (a non-UTF-8 directory), so
            // run jj inside it instead of widening the sample to the workspace: sibling
            // projects run concurrently, and their changes must not fail this one's hooks.
            None => {
                cmd.current_dir(scope_path).arg(CWD_FILESET);
            }
        },
        None => {
            cmd.current_dir(root);
        }
    }
    let output = cmd.check(true).output().await?;
    Ok(output.stdout)
}

/// Command that prints the working copy's patch, for `--show-diff-on-failure`.
///
/// The caller controls the colors, which `jj_cmd` pins off for parsing, so this is the
/// one jj command that builds its own `Cmd`. It runs in `root` and scopes the patch there,
/// like the Git command it replaces: an unscoped diff would cover the whole Jujutsu
/// workspace, including projects the run is not about.
pub(crate) fn diff_worktree_cmd(root: &Path, color: &str) -> Result<Cmd, Error> {
    let mut cmd = Cmd::new(JJ.as_ref().map_err(|&e| Error::JjNotFound(e))?);
    cmd.current_dir(root)
        .arg("--no-pager")
        .arg(color)
        .arg("diff")
        .arg("--git")
        .arg("-r")
        .arg("@")
        .arg(CWD_FILESET);
    Ok(cmd)
}

/// Files newly added in the current working-copy revision (absent before), relative to `cwd`.
///
/// Mirrors `git::staged_added_files`: only files absent in the parent, scoped under
/// `cwd`, so hooks like `check-added-large-files` and `check-case-conflict` (which
/// compare against `cwd`-relative filenames) match in nested projects.
#[instrument(level = "trace")]
pub(crate) async fn get_added_files(root: &Path, cwd: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = jj_cmd()?
        .current_dir(cwd)
        .arg("diff")
        .arg("-r")
        .arg("@")
        .arg("-T")
        .arg(DIFF_TEMPLATE)
        // Scope to the `cwd` subtree so out-of-project files are excluded, matching
        // git's `--relative`.
        .arg(CWD_FILESET)
        .check(true)
        .output()
        .await?;
    Ok(parse_diff_entries(&output.stdout)
        .into_iter()
        // jj reports a copy whose source also changed as `C`; its destination is new,
        // and Git without copy detection reports the same file as an addition.
        .filter(|entry| matches!(entry.status, b'A' | b'C'))
        .filter_map(|entry| rebase_to_cwd(root, cwd, entry.path))
        .collect())
}

/// Files changed in the commit that was just completed (`--last-commit`).
///
/// `jj commit` leaves `@` as a new empty working-copy commit, so the completed commit is
/// `@-`, diffed against its first parent (Git's `HEAD~1`).
#[instrument(level = "trace")]
pub(crate) async fn get_last_commit_files(
    root: &Path,
    scope: Option<&Path>,
    include_deleted: bool,
) -> Result<Vec<FileEntry>, Error> {
    get_changed_files_between("first_parent(@-)", "@-", root, scope, include_deleted).await
}

/// The commit ID a Git-running hook can use for `reference`.
///
/// Git resolves the refs `pre-push` passes on its own, so a revision jj does not know is
/// returned unchanged. `None` is for a revision Git cannot use either: jj's root commit, whose
/// all-zero ID a diff range rejects, or a revset naming several commits (`@-` in a merge), where
/// there is no single ID to hand over.
#[instrument(level = "trace")]
pub(crate) async fn hook_commit_id(reference: &str) -> Result<Option<String>, Error> {
    let output = jj_cmd()?
        .arg("log")
        .arg("--no-graph")
        .arg("-r")
        .arg(translate_rev(reference))
        .arg("-T")
        // A separator, or several commits arrive as one run of concatenated IDs.
        .arg(r#"commit_id ++ "\n""#)
        .check(false)
        .output()
        .await?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let ids = stdout
        .lines()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .collect::<Vec<_>>();

    match ids.as_slice() {
        [] => Ok(Some(reference.to_owned())),
        [id] if id.bytes().all(|byte| byte == b'0') => Ok(None),
        [id] => Ok(Some((*id).to_owned())),
        _ => Ok(None),
    }
}

/// Map one Git ref onto its Jujutsu equivalent, so no `HEAD` revset alias is required.
///
/// `HEAD~1` is Git's first parent, while jj's `-` suffix selects every parent of a merge,
/// so the parent is spelled out with `first_parent`.
fn translate_rev(rev: &str) -> &str {
    match rev {
        "HEAD" => "@",
        "HEAD~1" => "first_parent(@)",
        _ => rev,
    }
}

/// Get the list of changed files between two Jujutsu revisions, relative to the
/// workspace root.
///
/// This mirrors the Git backend's `old...new` (merge-base) semantics, which is what
/// `--from-ref`/`--to-ref` document: diff from the fork point of the two revisions to
/// `new`, so edits made only on `old` since it diverged are not included.
#[instrument(level = "trace")]
pub(crate) async fn get_changed_files_between(
    old: &str,
    new: &str,
    root: &Path,
    scope: Option<&Path>,
    include_deleted: bool,
) -> Result<Vec<FileEntry>, Error> {
    let (old, new) = (translate_rev(old), translate_rev(new));

    let from = format!("fork_point(({old}) | ({new}))");
    let mut cmd = jj_cmd()?;
    cmd.current_dir(root)
        .arg("diff")
        .arg("--from")
        .arg(&from)
        .arg("--to")
        .arg(new)
        .arg("-T")
        .arg(DIFF_TEMPLATE);
    if let Some(scope_path) = scope
        && let Some(fileset) = scope_fileset(root, scope_path)
    {
        cmd.arg(fileset);
    }
    let output = cmd.check(true).output().await?;
    Ok(diff_file_entries(&output.stdout, include_deleted))
}

/// Get files with unresolved conflicts in the current Jujutsu working copy.
///
/// `jj resolve --list` renders a display table rather than machine-readable output, so
/// ask `jj file list` for the conflicted paths directly.
#[instrument(level = "trace")]
pub(crate) async fn get_conflicted_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = jj_cmd()?
        .current_dir(root)
        .arg("file")
        .arg("list")
        .arg("-T")
        .arg(CONFLICT_TEMPLATE)
        .check(true)
        .output()
        .await?;
    Ok(parse_path_records(&output.stdout))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_workspace_root_returns_none_for_non_jj_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_workspace_root(dir.path()).is_none());
    }

    #[test]
    fn find_workspace_root_finds_current_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::create_dir(dir.path().join(".jj")).unwrap();
        let result = find_workspace_root(dir.path());
        assert_eq!(result, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_workspace_root_finds_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::create_dir(dir.path().join(".jj")).unwrap();
        let child = dir.path().join("subdir");
        fs_err::create_dir(&child).unwrap();
        let result = find_workspace_root(&child);
        assert_eq!(result, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn resolve_backing_git_dir_returns_none_without_repo_metadata() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::create_dir(dir.path().join(".jj")).unwrap();
        let result = resolve_backing_git_dir(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn resolve_backing_git_dir_resolves_colocated_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let store_dir = root.join(".jj").join("repo").join("store");
        fs_err::create_dir_all(&store_dir).unwrap();
        fs_err::write(store_dir.join("git_target"), "../../../.git").unwrap();
        let git_dir = root.join(".git");
        fs_err::create_dir(&git_dir).unwrap();

        let resolved = resolve_backing_git_dir(root).unwrap();
        assert_eq!(resolved, Some(dunce::canonicalize(&git_dir).unwrap()));
    }

    #[test]
    fn resolve_backing_git_dir_resolves_secondary_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let main_root = dir.path().join("main");
        let secondary_root = dir.path().join("secondary");

        let main_store = main_root.join(".jj").join("repo").join("store");
        fs_err::create_dir_all(&main_store).unwrap();
        fs_err::write(main_store.join("git_target"), "../../../.git").unwrap();
        let main_git = main_root.join(".git");
        fs_err::create_dir(&main_git).unwrap();

        let secondary_jj = secondary_root.join(".jj");
        fs_err::create_dir_all(&secondary_jj).unwrap();
        let main_repo_abs = dunce::canonicalize(main_root.join(".jj").join("repo")).unwrap();
        fs_err::write(
            secondary_jj.join("repo"),
            main_repo_abs.to_string_lossy().as_ref(),
        )
        .unwrap();

        let resolved = resolve_backing_git_dir(&secondary_root).unwrap();
        assert_eq!(resolved, Some(dunce::canonicalize(&main_git).unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn resolve_backing_git_dir_keeps_metadata_paths_verbatim() {
        // jj writes these files without a line terminator, and a path may end in spaces.
        use std::os::unix::ffi::OsStrExt as _;

        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store_dir = root.join(".jj").join("repo").join("store");
        fs_err::create_dir_all(&store_dir).unwrap();
        let target = root.join("git dir ");
        fs_err::create_dir(&target).unwrap();
        fs_err::write(store_dir.join("git_target"), target.as_os_str().as_bytes()).unwrap();

        let resolved = resolve_backing_git_dir(root).unwrap();
        assert_eq!(resolved, Some(dunce::canonicalize(&target).unwrap()));
    }

    #[test]
    fn resolve_backing_git_dir_returns_none_without_git_target() {
        // A non-Git jj backend has a store directory but no `git_target` file.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs_err::create_dir_all(root.join(".jj").join("repo").join("store")).unwrap();

        let resolved = resolve_backing_git_dir(root).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn resolve_backing_git_dir_returns_none_for_dangling_git_target() {
        // `git_target` points at a path that does not exist; this should be reported
        // as "no backing dir", not surface as an error from `canonicalize`.
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let store_dir = root.join(".jj").join("repo").join("store");
        fs_err::create_dir_all(&store_dir).unwrap();
        fs_err::write(store_dir.join("git_target"), "../../../.git").unwrap();
        // Note: no `.git` directory is created.

        let resolved = resolve_backing_git_dir(root).unwrap();
        assert!(resolved.is_none());
    }

    #[test]
    fn literal_fileset_quotes_paths() {
        assert_eq!(
            literal_fileset(Path::new("foo.txt")).as_deref(),
            Some(r#"cwd:"foo.txt""#)
        );
        assert_eq!(
            literal_fileset(Path::new(".")).as_deref(),
            Some(r#"cwd:".""#)
        );
        // Fileset metacharacters must be preserved literally, not treated as globs.
        assert_eq!(
            literal_fileset(Path::new("glob[1].txt")).as_deref(),
            Some(r#"cwd:"glob[1].txt""#)
        );
    }

    #[test]
    fn literal_fileset_escapes_only_quotes_and_backslashes() {
        // jj understands `\\` and `\"` and nothing else, so other characters are left
        // literal rather than escaped the way Rust's debug formatting would.
        assert_eq!(
            literal_fileset(Path::new("a\"b.txt")).as_deref(),
            Some(r#"cwd:"a\"b.txt""#)
        );
        assert_eq!(
            literal_fileset(Path::new("zero\u{200b}width")).as_deref(),
            Some("cwd:\"zero\u{200b}width\"")
        );
    }

    #[test]
    fn scope_fileset_uses_dot_for_the_scoped_root() {
        // A fileset path is cwd-relative, so scoping to the command directory itself
        // must be `.`; an absolute path would match nothing.
        assert_eq!(
            scope_fileset(Path::new("/repo"), Path::new("/repo")).as_deref(),
            Some(r#"cwd:".""#)
        );
        assert_eq!(
            scope_fileset(Path::new("/repo"), Path::new("/repo/project")).as_deref(),
            Some(r#"cwd:"project""#)
        );
    }

    #[test]
    fn scope_fileset_keeps_relative_paths_relative() {
        // Pathspecs from `--directory` and globs are already relative to the command
        // directory, and must scope the query rather than widening it to the whole tree.
        assert_eq!(
            scope_fileset(Path::new("/repo"), Path::new("src")).as_deref(),
            Some(r#"cwd:"src""#)
        );
        assert_eq!(
            scope_fileset(Path::new("/repo"), Path::new(".")).as_deref(),
            Some(r#"cwd:".""#)
        );
        // A rooted path outside the command directory falls back to the whole tree
        // rather than emitting a fileset that matches nothing.
        assert_eq!(
            scope_fileset(Path::new("/repo"), Path::new("/elsewhere")).as_deref(),
            Some(r#"cwd:".""#)
        );
    }

    #[cfg(unix)]
    #[test]
    fn scope_fileset_declines_paths_without_a_utf8_spelling() {
        // jj filesets are text, so a non-UTF-8 directory cannot be named in one; the
        // caller queries a wider scope and filters the decoded paths instead.
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let path = Path::new(OsStr::from_bytes(b"proj-\xff"));
        assert_eq!(literal_fileset(path), None);
        assert_eq!(scope_fileset(Path::new("/repo"), path), None);
    }

    #[test]
    fn translate_rev_maps_git_defaults_onto_revsets() {
        // `HEAD` is the working-copy commit, and `HEAD~1` its first parent (`-` alone would
        // select every parent of a merge).
        assert_eq!(translate_rev("HEAD"), "@");
        assert_eq!(translate_rev("HEAD~1"), "first_parent(@)");
        // Anything else is already a revset or a bookmark.
        assert_eq!(translate_rev("main"), "main");
    }

    #[test]
    fn rebase_to_cwd_strips_the_project_prefix() {
        assert_eq!(
            rebase_to_cwd(
                Path::new("/repo"),
                Path::new("/repo"),
                PathBuf::from("project/file.rs")
            ),
            Some(PathBuf::from("project/file.rs"))
        );
        assert_eq!(
            rebase_to_cwd(
                Path::new("/repo"),
                Path::new("/repo/project"),
                PathBuf::from("project/file.rs")
            ),
            Some(PathBuf::from("file.rs"))
        );
        // Paths outside the query directory are dropped.
        assert_eq!(
            rebase_to_cwd(
                Path::new("/repo"),
                Path::new("/repo/project"),
                PathBuf::from("other/file.rs")
            ),
            None
        );
    }

    #[test]
    fn parse_diff_entries_reads_status_types_and_path() {
        // Renames render as the real target path (not the `{a => b}` compaction), and a
        // path may contain spaces or newlines.
        let entries = parse_diff_entries(
            b"A  false added.txt\0M file true modified.txt\0D file false deleted.txt\0R file true dir/renamed.txt\0A  false with\nnewline.txt\0",
        );
        assert_eq!(entries.len(), 5);
        assert_eq!(entries[0].status, b'A');
        assert_eq!(entries[0].source_type, b"");
        assert!(!entries[0].source_executable);
        assert_eq!(entries[0].path, PathBuf::from("added.txt"));
        assert_eq!(entries[1].source_type, b"file");
        assert!(entries[1].source_executable);
        assert_eq!(entries[3].status, b'R');
        assert_eq!(entries[3].path, PathBuf::from("dir/renamed.txt"));
        assert_eq!(entries[4].path, PathBuf::from("with\nnewline.txt"));
    }

    #[test]
    fn diff_file_entries_reports_deleted_modes() {
        let entries = diff_file_entries(
            b"A  false added.txt\0M file false modified.txt\0D file false deleted.txt\0D file true deleted.sh\0D symlink false deleted-link\0D git-submodule false deleted-sub\0",
            true,
        );
        assert_eq!(entries.len(), 6);
        assert!(entries[0].deleted_mode.is_none());
        assert!(matches!(entries[2].deleted_mode, Some(FileMode::Regular)));
        assert!(matches!(
            entries[3].deleted_mode,
            Some(FileMode::Executable)
        ));
        assert!(matches!(entries[4].deleted_mode, Some(FileMode::Symlink)));
        assert!(matches!(entries[5].deleted_mode, Some(FileMode::Submodule)));
    }

    #[test]
    fn diff_file_entries_excludes_deletions_by_default() {
        let entries = diff_file_entries(b"A  false added.txt\0D file false deleted.txt\0", false);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].path, PathBuf::from("added.txt"));
    }

    #[test]
    fn diff_file_entries_reports_rename_source_as_a_deletion() {
        // A rename emits its destination, plus the removed source path as a deletion
        // (the record `DIFF_TEMPLATE` adds for renames).
        let output = b"R file true renamed.sh\0D file true script.sh\0";
        assert_eq!(diff_file_entries(output, false).len(), 1);
        let entries = diff_file_entries(output, true);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].path, PathBuf::from("renamed.sh"));
        assert!(entries[0].deleted_mode.is_none());
        assert_eq!(entries[1].path, PathBuf::from("script.sh"));
        assert!(matches!(
            entries[1].deleted_mode,
            Some(FileMode::Executable)
        ));
    }
}
