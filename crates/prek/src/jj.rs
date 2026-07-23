use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tracing::instrument;

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

fn resolve_repo_dir(workspace_root: &Path) -> Result<Option<PathBuf>, Error> {
    let repo_dir_candidate = workspace_root.join(".jj").join("repo");
    if repo_dir_candidate.is_file() {
        let content = fs_err::read_to_string(&repo_dir_candidate)?;
        let path = PathBuf::from(content.trim());
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
        let git_target = fs_err::read_to_string(&git_target_file)?;
        let git_target = git_target.trim();

        let git_path = PathBuf::from(git_target);
        if git_path.is_absolute() {
            git_path
        } else {
            repo_dir.join("store").join(git_path)
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

/// Create a new `Cmd` for running Jujutsu.
pub(crate) fn jj_cmd() -> Result<Cmd, Error> {
    let cmd = Cmd::new(JJ.as_ref().map_err(|&e| Error::JjNotFound(e))?);
    Ok(cmd)
}

fn relative_to_cwd<'a>(cwd: &'a Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(cwd)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(path)
}

/// Template that makes `jj file list` print one path per line regardless of the
/// user's `templates.file_list` configuration.
const FILE_LIST_TEMPLATE: &str = r#"path ++ "\n""#;

fn parse_path_lines(output: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(output)
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// Build a jj fileset expression matching `path` literally, relative to the
/// command's working directory. jj interprets bare path arguments as fileset
/// expressions, so metacharacters like `[` must be quoted; this mirrors git's
/// `--literal-pathspecs`.
fn literal_fileset(path: &Path) -> String {
    // jj uses forward slashes in filesets, and string literals use double quotes
    // with backslash escaping (matching Rust's debug formatting).
    let path = path.to_string_lossy().replace('\\', "/");
    format!("cwd:{path:?}")
}

/// Build a literal fileset that scopes a query to `path`, expressed relative to `cwd`.
fn scope_fileset(cwd: &Path, path: &Path) -> String {
    literal_fileset(relative_to_cwd(cwd, path))
}

/// A single entry from `jj diff --types`: the file's kind before and after the
/// change, plus its path (relative to the command's working directory). The kind
/// bytes are jj's status letters, e.g. `F` file, `L` symlink, `C` conflict, `-`
/// absent.
struct TypedChange {
    before: u8,
    after: u8,
    path: PathBuf,
}

fn parse_typed_changes(output: &[u8]) -> Vec<TypedChange> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let (types, path) = line.trim_end().split_once(char::is_whitespace)?;
            let bytes = types.as_bytes();
            if bytes.len() < 2 {
                return None;
            }
            let path = path.trim_start();
            if path.is_empty() {
                return None;
            }
            Some(TypedChange {
                before: bytes[0],
                after: bytes[1],
                path: PathBuf::from(path),
            })
        })
        .collect()
}

/// List tracked files in the current Jujutsu workspace revision under `paths`.
///
/// Paths are interpreted relative to `cwd`, mirroring `git::ls_files`. `jj file list`
/// prints paths relative to `cwd`, so the results line up with the Git backend.
#[instrument(level = "trace", skip(paths))]
pub(crate) async fn ls_files<P>(
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
        cmd.arg(scope_fileset(cwd, path.as_ref()));
    }

    let output = cmd.check(true).output().await?;
    Ok(parse_path_lines(&output.stdout))
}

/// Files changed in the current working-copy revision, excluding deletions.
///
/// Deletions are dropped to match the Git backend, whose `--diff-filter` never
/// reports deleted paths (running hooks on nonexistent files is pointless).
#[instrument(level = "trace")]
pub(crate) async fn get_changed_files(
    root: &Path,
    scope: Option<&Path>,
) -> Result<Vec<PathBuf>, Error> {
    let mut cmd = jj_cmd()?;
    cmd.current_dir(root)
        .arg("diff")
        .arg("-r")
        .arg("@")
        .arg("--types");
    if let Some(scope_path) = scope {
        cmd.arg(scope_fileset(root, scope_path));
    }
    let output = cmd.check(true).output().await?;
    Ok(changed_paths(&output.stdout))
}

/// Files newly added in the current working-copy revision (absent before).
///
/// Mirrors `git diff --staged --diff-filter=A`: only files that did not exist in
/// the parent, so hooks like `check-added-large-files` do not flag pre-existing
/// files that were merely modified.
#[instrument(level = "trace")]
pub(crate) async fn get_added_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = jj_cmd()?
        .current_dir(root)
        .arg("diff")
        .arg("-r")
        .arg("@")
        .arg("--types")
        .check(true)
        .output()
        .await?;
    Ok(parse_typed_changes(&output.stdout)
        .into_iter()
        .filter(|change| change.before == b'-' && change.after != b'-')
        .map(|change| change.path)
        .collect())
}

/// Get the list of changed files between two Jujutsu revisions (excluding deletions).
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
) -> Result<Vec<PathBuf>, Error> {
    // Map git's "HEAD" to jj's "@" for the working-copy commit.
    let old = match old {
        "HEAD" => "@",
        "HEAD~1" => "@-",
        _ => old,
    };
    let new = match new {
        "HEAD" => "@",
        "HEAD~1" => "@-",
        _ => new,
    };

    let from = format!("fork_point(({old}) | ({new}))");
    let mut cmd = jj_cmd()?;
    cmd.current_dir(root)
        .arg("diff")
        .arg("--from")
        .arg(&from)
        .arg("--to")
        .arg(new)
        .arg("--types");
    if let Some(scope_path) = scope {
        cmd.arg(scope_fileset(root, scope_path));
    }
    let output = cmd.check(true).output().await?;
    Ok(changed_paths(&output.stdout))
}

fn changed_paths(output: &[u8]) -> Vec<PathBuf> {
    parse_typed_changes(output)
        .into_iter()
        .filter(|change| change.after != b'-')
        .map(|change| change.path)
        .collect()
}

/// Get files with unresolved conflicts in the current Jujutsu working copy.
///
/// Uses `jj resolve --list`, which is the authoritative list of unresolved
/// conflicts. It exits with code 2 and a specific stderr message when there
/// are none, which we handle; other errors are propagated.
#[instrument(level = "trace")]
pub(crate) async fn get_conflicted_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = jj_cmd()?
        .current_dir(root)
        .arg("resolve")
        .arg("--list")
        .check(false)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // jj resolve --list returns exit code 2 and "No conflicts found" on success-no-conflicts.
        if output.status.code() == Some(2) && stderr.contains("No conflicts found") {
            return Ok(Vec::new());
        }
        return Err(process::Error::Status {
            command: "jj resolve --list".to_string(),
            error: process::StatusError {
                status: output.status,
                output: Some(output),
            },
        }
        .into());
    }

    Ok(parse_conflict_list(&output.stdout))
}

/// Parse `jj resolve --list` output into conflicted paths.
///
/// Each line is `<path><padding><N>-sided conflict`, where `<padding>` is a run of
/// two or more spaces. Split on the LAST such run so a path that itself contains
/// spaces (even consecutive ones) is preserved.
fn parse_conflict_list(output: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(output)
        .lines()
        .filter_map(|line| {
            let line = line.trim_end();
            let path = line
                .rsplit_once("  ")
                .map_or(line, |(path, _)| path)
                .trim_end();
            if path.is_empty() {
                None
            } else {
                Some(PathBuf::from(path))
            }
        })
        .collect()
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
        assert_eq!(literal_fileset(Path::new("foo.txt")), r#"cwd:"foo.txt""#);
        assert_eq!(literal_fileset(Path::new(".")), r#"cwd:".""#);
        // Fileset metacharacters must be preserved literally, not treated as globs.
        assert_eq!(
            literal_fileset(Path::new("glob[1].txt")),
            r#"cwd:"glob[1].txt""#
        );
    }

    #[test]
    fn parse_typed_changes_reads_status_and_path() {
        let changes = parse_typed_changes(b"-F added.txt\nFF modified.txt\nF- deleted.txt\n");
        assert_eq!(changes.len(), 3);
        assert_eq!(changes[0].before, b'-');
        assert_eq!(changes[0].after, b'F');
        assert_eq!(changes[0].path, PathBuf::from("added.txt"));
        assert_eq!(changes[2].before, b'F');
        assert_eq!(changes[2].after, b'-');
    }

    #[test]
    fn changed_paths_excludes_deletions() {
        let paths = changed_paths(b"-F added.txt\nFF modified.txt\nF- deleted.txt\n");
        assert_eq!(
            paths,
            vec![PathBuf::from("added.txt"), PathBuf::from("modified.txt")]
        );
    }

    #[test]
    fn parse_conflict_list_extracts_paths_with_spaces() {
        // `jj resolve --list` pads the path with 2+ spaces before the description.
        let out = b"src/a.txt    2-sided conflict\nwith space.txt    2-sided conflict\ntwo  spaces.txt    3-sided conflict\n";
        let paths = parse_conflict_list(out);
        assert_eq!(
            paths,
            vec![
                PathBuf::from("src/a.txt"),
                PathBuf::from("with space.txt"),
                PathBuf::from("two  spaces.txt"),
            ]
        );
    }
}
