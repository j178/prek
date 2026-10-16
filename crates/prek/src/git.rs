use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::Utf8Error;
use std::sync::{LazyLock, OnceLock};

use anyhow::Result;
use itertools::Itertools;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use rustc_hash::FxHashSet;
use same_file::is_same_file;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, instrument, warn};

use crate::fs::{CWD, PathClean};
use crate::jj;
use crate::process;
use crate::process::{Cmd, StatusError};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Command(#[from] process::Error),

    #[error("Failed to find git: {0}")]
    GitNotFound(#[from] which::Error),

    #[error(
        "Not in a Git repository. Change to a Git repository, or run `git init` to create one."
    )]
    NotRepository,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Jujutsu(#[from] jj::Error),

    #[error(
        "Detected a Jujutsu workspace at `{0}`, but prek could not find the Git repository backing it"
    )]
    JujutsuWithoutGitStore(PathBuf),

    #[error(transparent)]
    UTF8(#[from] Utf8Error),

    #[error("Invalid file record in Git diff output")]
    InvalidDiffFile,

    #[error(
        "Git resolved hooks directory to the current directory (`{0}`). Unset `core.hooksPath` or set it to a real directory path."
    )]
    InvalidHooksPath(PathBuf),
}

pub(crate) static GIT: LazyLock<Result<PathBuf, which::Error>> = LazyLock::new(|| {
    let git = which::which("git")?;
    // Resolve Apple's developer-tool shim once instead of looking up the toolchain
    // on every Git invocation. Leave custom Git executables and wrappers alone.
    #[cfg(target_os = "macos")]
    if git == Path::new("/usr/bin/git")
        && let Ok(output) = Command::new("/usr/bin/xcrun")
            .args(["--find", "git"])
            .output()
        && output.status.success()
    {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let stdout = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
        return Ok(PathBuf::from(OsStr::from_bytes(stdout)));
    }
    Ok(git)
});

// Git can expose `GIT_DIR` without `GIT_WORK_TREE` to hooks. Keep the derived
// work tree in process-local state and add it only when preserving the current
// repository requires it.
static GIT_WORK_TREE: OnceLock<Option<PathBuf>> = OnceLock::new();

pub(crate) fn init_git_work_tree() -> Result<()> {
    if !EnvVars.is_set(EnvVars::GIT_DIR) || EnvVars.is_set(EnvVars::GIT_WORK_TREE) {
        let _ = GIT_WORK_TREE.set(None);
        return Ok(());
    }

    let cwd = std::env::current_dir()?;
    debug!(
        "Using {} `{}` for git commands",
        EnvVars::GIT_WORK_TREE,
        cwd.display()
    );
    let _ = GIT_WORK_TREE.set(Some(cwd));
    Ok(())
}

/// Return the absolute worktree path saved for hook subprocesses, if one was needed.
fn git_work_tree() -> Option<&'static Path> {
    GIT_WORK_TREE.get().and_then(Option::as_deref)
}

#[derive(Debug)]
struct Repo {
    root: Result<PathBuf, Error>,
    git_dir: PathBuf,
    common_dir: PathBuf,
    hooks_dir: PathBuf,
    /// Whether the boundary came from a Jujutsu workspace.
    jujutsu: bool,
    /// Backing Git store to point Git at, set only when Git cannot find the repository
    /// on its own (a Jujutsu workspace with no `.git` entry at or below its root).
    backing_store: Option<jj::GitStore>,
}

static REPO: LazyLock<Result<Repo, Error>> =
    LazyLock::new(|| Repo::discover(&std::env::current_dir()?));

/// Return the absolute path of the current repository's working tree.
pub(crate) fn root() -> Result<&'static Path, &'static Error> {
    REPO.as_ref()?.root.as_deref()
}

/// Whether the repository boundary came from a Jujutsu workspace.
pub(crate) fn is_jujutsu() -> bool {
    REPO.as_ref().is_ok_and(|repo| repo.jujutsu)
}

/// Return the absolute Git directory of the current worktree, even after changing directory.
pub(crate) fn git_dir() -> Result<&'static Path, &'static Error> {
    Ok(&REPO.as_ref()?.git_dir)
}

/// Return the absolute Git directory shared by the current repository's worktrees.
pub(crate) fn common_dir() -> Result<&'static Path, &'static Error> {
    Ok(&REPO.as_ref()?.common_dir)
}

/// Repository-local environment variables cleared before operating on another repository.
///
/// `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_*`, and `GIT_CONFIG_VALUE_*`
/// are deliberately excluded so nested Git commands retain caller-supplied command-scoped settings.
/// The repository variables that name a path: a command changing directory has to anchor them
/// to the one prek was started in first.
static GIT_REPO_PATH_ENVS: &[&str] = &[
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
    "GIT_OBJECT_DIRECTORY",
];

static GIT_REPO_LOCAL_ENVS: &[&str] = &[
    "GIT_ALTERNATE_OBJECT_DIRECTORIES",
    "GIT_CONFIG",
    "GIT_OBJECT_DIRECTORY",
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_IMPLICIT_WORK_TREE",
    "GIT_GRAFT_FILE",
    "GIT_INDEX_FILE",
    "GIT_NO_REPLACE_OBJECTS",
    "GIT_REPLACE_REF_BASE",
    "GIT_PREFIX",
    "GIT_INTERNAL_SUPER_PREFIX",
    "GIT_SHALLOW_FILE",
    "GIT_COMMON_DIR",
];

pub(crate) trait GitCommandExt {
    /// Keep nested Git commands operating on the repository that invoked the hook.
    ///
    /// Git treats the current directory as the work tree when `GIT_DIR` is set but
    /// `GIT_WORK_TREE` is not. If prek starts the hook in a subdirectory, this method passes
    /// the original work tree explicitly so Git commands in the hook still use the repository
    /// that invoked prek.
    ///
    /// A hook that intentionally operates on another repository must clear Git's
    /// repository-local environment before starting Git.
    fn preserve_current_worktree(&mut self, hook_cwd: &Path) -> &mut Self;

    fn sanitize_git_repo_env(&mut self) -> &mut Self;
}

/// The Jujutsu backing store to expose to a command running in `dir`, if the active
/// repository needs one and `dir` lies in the workspace above any nested repository.
///
/// Only a workspace with no `.git` entry of its own needs this: Git commands started
/// elsewhere (hook environments, dependency repositories, nested Git repositories) must
/// not inherit it.
fn backing_store_for(dir: &Path) -> Option<&'static jj::GitStore> {
    let store = REPO.as_ref().ok()?.backing_store.as_ref()?;
    let dir = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
    if !dir.starts_with(&store.workspace_root) {
        return None;
    }

    // A `.git` between `dir` and the workspace root marks a nested repository, whose
    // own Git commands should find it rather than the workspace's backing store.
    let mut ancestor = dir.as_path();
    while ancestor != store.workspace_root {
        if ancestor.join(".git").exists() {
            return None;
        }
        ancestor = ancestor.parent()?;
    }
    Some(store)
}

pub(crate) fn apply_git_work_tree(cmd: &mut Command) -> &mut Command {
    if let Some(work_tree) = git_work_tree() {
        cmd.env(EnvVars::GIT_WORK_TREE, work_tree);
    }
    cmd
}

impl GitCommandExt for Cmd {
    fn preserve_current_worktree(&mut self, hook_cwd: &Path) -> &mut Self {
        // A hook's own Git commands have to find the repository, which a Jujutsu
        // workspace with no `.git` of its own cannot do on its own.
        if let Some(store) = backing_store_for(hook_cwd) {
            self.env(EnvVars::GIT_DIR, &store.git_dir);
            self.env(EnvVars::GIT_WORK_TREE, &store.workspace_root);
            return self;
        }

        let Some(work_tree) = git_work_tree() else {
            return self;
        };
        let is_work_tree_root =
            hook_cwd == work_tree || is_same_file(hook_cwd, work_tree).unwrap_or(false);
        if !is_work_tree_root {
            self.env(EnvVars::GIT_WORK_TREE, work_tree);
        }
        self
    }

    fn sanitize_git_repo_env(&mut self) -> &mut Self {
        for key in GIT_REPO_LOCAL_ENVS {
            self.env_remove(key);
        }
        self
    }
}

/// Build a Git command with prek's baseline flags, but without pointing it at a
/// Jujutsu workspace's backing store.
///
/// Discovery is what resolves that store in the first place, so it must never consult
/// it, and neither must anything it calls.
fn raw_git_cmd() -> Result<Cmd, Error> {
    let mut cmd = Cmd::new(GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?);
    cmd.hidden_args(["-c", "core.useBuiltinFSMonitor=false"]);
    if let Some(work_tree) = git_work_tree() {
        cmd.env(EnvVars::GIT_WORK_TREE, work_tree);
    }
    Ok(cmd)
}

pub(crate) fn git_cmd() -> Result<Cmd, Error> {
    let mut cmd = raw_git_cmd()?;
    // A Jujutsu workspace with no `.git` of its own has to be driven through the
    // backing store resolved from its `.jj` metadata. Colocated workspaces need no
    // injection: Git finds the repository from the workspace itself, and pointing it
    // elsewhere would lose a secondary workspace's own Git worktree.
    //
    // Commands that query a *different* repository (as `try-repo` does) must drop this
    // again; see `git_cmd_isolated`.
    if let Ok(repo) = REPO.as_ref()
        && let Some(store) = &repo.backing_store
    {
        cmd.env(EnvVars::GIT_DIR, &store.git_dir);
        cmd.env(EnvVars::GIT_WORK_TREE, &store.workspace_root);
    }

    Ok(cmd)
}

/// A Git command detached from prek's own repository environment.
///
/// The repository-local environment includes the backing store injected for a
/// Jujutsu workspace, so commands that intentionally target another repository must
/// clear it.
pub(crate) fn git_cmd_isolated() -> Result<Cmd, Error> {
    let mut cmd = git_cmd()?;
    cmd.sanitize_git_repo_env();
    Ok(cmd)
}

/// The Git root `dir` belongs to, asked through the repository selection prek inherited and
/// without any Jujutsu backing store.
pub(crate) async fn root_at(dir: &Path) -> Result<Option<PathBuf>, Error> {
    let output = git_cmd_for_dir()?
        .current_dir(dir)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .check(false)
        .output()
        .await?;

    if !output.status.success() {
        return Ok(None);
    }
    // Only Git's own trailing newline: a directory name may end in whitespace, and a path may
    // hold bytes that are not UTF-8.
    let stdout = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
    Ok(Some(path_from_git_bytes(stdout)?))
}

/// A Git command addressed to the repository a *directory* belongs to, rather than to the one
/// prek resolved.
///
/// `git_cmd` points Git at a Jujutsu workspace's backing store, which would answer for the
/// workspace instead of for a Git repository nested inside it, and `git_cmd_isolated` also drops
/// a repository the caller selected with `GIT_DIR` and friends. Putting those back leaves the
/// command with the environment prek inherited and nothing else.
pub(crate) fn git_cmd_for_dir() -> Result<Cmd, Error> {
    // The unflagged builder keeps the environment prek inherited, including the work tree
    // `init_git_work_tree` saved for a `GIT_DIR` that came without one, and adds no backing store.
    let mut cmd = raw_git_cmd()?;
    // A relative repository path belongs to the directory prek was started in, while these
    // commands change into a project directory before asking Git about it.
    for key in GIT_REPO_PATH_ENVS {
        if let Some(value) = EnvVars.var_os(key)
            && Path::new(&value).is_relative()
        {
            cmd.env(key, CWD.join(value));
        }
    }
    Ok(cmd)
}

/// Decode NUL-separated paths without changing their spelling or base directory.
fn zsplit(s: &[u8]) -> Result<Vec<PathBuf>, Utf8Error> {
    s.split(|&b| b == b'\0')
        .filter(|slice| !slice.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

/// Decode a Git path without resolving it against a directory.
#[cfg(unix)]
#[expect(clippy::unnecessary_wraps)]
pub(crate) fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf, Utf8Error> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

/// Decode a Git path without resolving it against a directory.
#[cfg(not(unix))]
pub(crate) fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf, Utf8Error> {
    str::from_utf8(bytes).map(PathBuf::from)
}

/// Encode a path for Git without changing its spelling or base directory.
#[cfg(unix)]
#[expect(clippy::unnecessary_wraps)]
fn path_to_git_bytes(path: &Path) -> std::io::Result<&[u8]> {
    use std::os::unix::ffi::OsStrExt as _;

    Ok(path.as_os_str().as_bytes())
}

/// Encode a path for Git without changing its spelling or base directory.
#[cfg(not(unix))]
fn path_to_git_bytes(path: &Path) -> std::io::Result<&[u8]> {
    path.to_str().map(str::as_bytes).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Path is not valid UTF-8: `{}`", path.display()),
        )
    })
}

/// Return newly staged paths under `root`, relative to `root` (the hook's working directory).
///
/// For example, with `root = <repo>/project`, `<repo>/project/file.rs` is returned as `file.rs`.
pub(crate) async fn staged_added_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = git_cmd()?
        .current_dir(root)
        .arg("diff")
        .hidden_args(["--no-ext-diff"])
        .arg("--cached")
        .arg("--relative")
        .arg("--name-only")
        .arg("--diff-filter=A")
        .arg("-z")
        .check(true)
        .output()
        .await?;
    Ok(zsplit(&output.stdout)?)
}

/// Return changed paths relative to the repository root.
///
/// `root` selects the repository. Results cover the whole repository, even when
/// `root` is a subdirectory; each `FileEntry.path` retains its repository-relative prefix.
pub(crate) async fn changed_files(
    old: &str,
    new: &str,
    root: &Path,
    include_deleted: bool,
) -> Result<Vec<FileEntry>, Error> {
    // Try three-dot syntax first (merge-base diff), which works for commits
    let output = diff_files_cmd(include_deleted)?
        .current_dir(root)
        .arg(format!("{old}...{new}"))
        .check(false)
        .output()
        .await?;

    if output.status.success() {
        return parse_diff_files(&output.stdout);
    }

    // Fall back to two-dot syntax, which works with both commits and trees
    let output = diff_files_cmd(include_deleted)?
        .current_dir(root)
        .arg(format!("{old}..{new}"))
        .check(true)
        .output()
        .await?;
    parse_diff_files(&output.stdout)
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum FileMode {
    Regular,
    Executable,
    Symlink,
    Submodule,
}

pub(crate) struct FileEntry {
    pub(crate) path: PathBuf,
    pub(crate) deleted_mode: Option<FileMode>,
}

impl From<PathBuf> for FileEntry {
    fn from(path: PathBuf) -> Self {
        Self {
            path,
            deleted_mode: None,
        }
    }
}

fn diff_files_cmd(include_deleted: bool) -> Result<Cmd, Error> {
    let mut cmd = git_cmd()?;
    configure_diff_files(&mut cmd, include_deleted);
    Ok(cmd)
}

/// Configure a command to emit one raw diff record per changed path.
///
/// Each raw record must have one path. Renames become additions and deletions,
/// so old paths can trigger hooks independently of Git's similarity heuristics.
fn configure_diff_files(cmd: &mut Cmd, include_deleted: bool) -> &mut Cmd {
    cmd.args(["diff", "--raw", "--no-renames", "--no-relative", "-z"])
        .hidden_args(["--no-ext-diff"]);
    if !include_deleted {
        cmd.arg("--diff-filter=d");
    }
    cmd
}

/// Parse raw diff records without changing the paths' spelling or base directory.
/// Output from `diff_files_cmd` contains repository-relative paths.
fn parse_diff_files(output: &[u8]) -> Result<Vec<FileEntry>, Error> {
    let mut fields = output.split(|&byte| byte == b'\0');
    let mut files = Vec::new();
    while let Some(header) = fields.next().filter(|header| !header.is_empty()) {
        let deleted_mode = if header.ends_with(b" D") {
            Some(match header.split(|&byte| byte == b' ').next() {
                Some(b":100644") => FileMode::Regular,
                Some(b":100755") => FileMode::Executable,
                Some(b":120000") => FileMode::Symlink,
                Some(b":160000") => FileMode::Submodule,
                _ => return Err(Error::InvalidDiffFile),
            })
        } else {
            None
        };
        let path = fields
            .next()
            .filter(|path| !path.is_empty())
            .ok_or(Error::InvalidDiffFile)?;
        files.push(FileEntry {
            path: path_from_git_bytes(path)?,
            deleted_mode,
        });
    }
    Ok(files)
}

/// Return indexed paths matching `paths`, relative to `cwd`.
///
/// Relative input paths are also interpreted relative to `cwd`. Absolute input
/// paths still produce paths relative to `cwd`.
#[instrument(level = "trace", skip(paths))]
pub(crate) async fn ls_files<P>(
    cwd: &Path,
    paths: impl IntoIterator<Item = P>,
) -> Result<Vec<PathBuf>, Error>
where
    P: AsRef<Path>,
{
    let mut cmd = git_cmd()?;
    cmd.current_dir(cwd)
        .arg("--literal-pathspecs")
        .arg("ls-files")
        .arg("-z")
        .arg("--");
    for path in paths {
        cmd.arg(path.as_ref());
    }
    let output = cmd.check(true).output().await?;

    Ok(zsplit(&output.stdout)?)
}

/// Return the absolute hooks directory, including any `core.hooksPath` override.
pub(crate) async fn hooks_dir() -> Result<&'static Path> {
    let hooks_dir = &REPO.as_ref()?.hooks_dir;
    // `core.hooksPath=` is a particularly dangerous case: Git treats it as
    // configured, but resolves `--git-path hooks` to the current directory. If
    // we accepted that value, install/uninstall would write or remove hook
    // shims from the worktree root. Keep the explicit `core.hooksPath=.` case
    // working, but reject the empty-string variant.
    if hooks_dir.clean() == *crate::fs::CWD && config_value_is_empty(None, "core.hooksPath").await?
    {
        Err(Error::InvalidHooksPath(PathBuf::from(".")).into())
    } else {
        Ok(hooks_dir)
    }
}

/// Return staged paths relative to the repository root.
///
/// `root` selects the repository. Results cover the whole repository, even when
/// `root` is a subdirectory; each `FileEntry.path` retains its repository-relative prefix.
pub(crate) async fn staged_files(
    root: &Path,
    include_deleted: bool,
) -> Result<Vec<FileEntry>, Error> {
    staged_files_with(diff_files_cmd(include_deleted)?, root).await
}

/// Staged paths of another repository, isolated from prek's own repository environment.
///
/// `try-repo` queries repositories that are not the one prek was started in.
pub(crate) async fn staged_files_isolated(
    root: &Path,
    include_deleted: bool,
) -> Result<Vec<FileEntry>, Error> {
    let mut cmd = git_cmd_isolated()?;
    configure_diff_files(&mut cmd, include_deleted);
    staged_files_with(cmd, root).await
}

async fn staged_files_with(mut cmd: Cmd, root: &Path) -> Result<Vec<FileEntry>, Error> {
    let output = cmd
        .current_dir(root)
        .arg("--cached")
        .check(true)
        .output()
        .await?;
    parse_diff_files(&output.stdout)
}

pub(crate) struct WorktreeStatus {
    pub(crate) unmerged: bool,
    pub(crate) unstaged: Vec<PathBuf>,
    pub(crate) intent_to_add: Vec<PathBuf>,
}

/// Check conflicts and unstaged changes together. Paths are relative to the repository root.
pub(crate) async fn worktree_status(cwd: &Path) -> Result<WorktreeStatus, Error> {
    let output = git_cmd()?
        .current_dir(cwd)
        .args([
            "diff",
            "--name-status",
            "-z",
            "--no-renames",
            "--no-relative",
            "--no-ext-diff",
            "--ignore-submodules",
        ])
        .check(true)
        .output()
        .await?;
    parse_worktree_status(&output.stdout)
}

fn parse_worktree_status(output: &[u8]) -> Result<WorktreeStatus, Error> {
    let mut status = WorktreeStatus {
        unmerged: false,
        unstaged: Vec::new(),
        intent_to_add: Vec::new(),
    };
    // Disabling renames gives each status exactly one NUL-terminated, unquoted path.
    let mut fields = output.split(|&byte| byte == b'\0');
    while let Some(change) = fields.next().filter(|field| !field.is_empty()) {
        let path = fields
            .next()
            .filter(|path| !path.is_empty())
            .ok_or(Error::InvalidDiffFile)?;
        let path = path_from_git_bytes(path)?;
        match change {
            b"U" => status.unmerged = true,
            // Unstaged additions are intent-to-add entries; untracked files are omitted.
            b"A" => status.intent_to_add.push(path.clone()),
            _ => {}
        }
        status.unstaged.push(path);
    }
    Ok(status)
}

/// Check for changes against `rev` anywhere in the repository containing `path`.
///
/// `path` selects the repository without limiting the check to that directory. That
/// repository is not necessarily prek's own, so the check ignores prek's repository
/// environment (`try-repo` relies on this).
pub(crate) async fn has_diff(rev: &str, path: &Path) -> Result<bool> {
    let status = git_cmd_isolated()?
        .current_dir(path)
        .arg("diff")
        .hidden_args(["--no-ext-diff"])
        .arg("--quiet")
        .arg("--no-relative")
        .arg(rev)
        .check(false)
        .status()
        .await?;
    Ok(status.code() == Some(1))
}

pub(crate) fn is_in_merge_conflict() -> Result<bool> {
    let git_dir = git_dir()?;
    Ok(git_dir.join("MERGE_HEAD").try_exists()? && git_dir.join("MERGE_MSG").try_exists()?)
}

/// Return paths involved in the merge relative to the repository root.
///
/// `root` selects the repository. Both the diff and `MERGE_MSG` paths use the
/// repository root as their base, including when `root` is a subdirectory.
pub(crate) async fn conflicted_files(root: &Path) -> Result<Vec<PathBuf>> {
    let tree = git_cmd()?
        .current_dir(root)
        .arg("write-tree")
        .check(true)
        .output()
        .await?;

    let output = git_cmd()?
        .current_dir(root)
        .arg("diff")
        .arg("--name-only")
        .arg("--no-relative")
        .hidden_args(["--no-ext-diff"])
        .arg("-z")
        .arg("-m") // Show diffs for merge commits in the default format.
        .arg(str::from_utf8(&tree.stdout)?.trim_ascii())
        .arg("HEAD")
        .arg("MERGE_HEAD")
        .check(true)
        .output()
        .await?;

    Ok(zsplit(&output.stdout)?
        .into_iter()
        .chain(parse_merge_msg_for_conflicts().await?)
        .collect::<HashSet<PathBuf>>()
        .into_iter()
        .collect())
}

/// Return conflict paths recorded in `MERGE_MSG`, relative to the repository root.
async fn parse_merge_msg_for_conflicts() -> Result<Vec<PathBuf>> {
    let git_dir = git_dir()?;
    let merge_msg = git_dir.join("MERGE_MSG");
    let content = fs_err::tokio::read_to_string(&merge_msg).await?;
    let conflicts = content
        .lines()
        // Conflicted files start with tabs
        .filter(|line| line.starts_with('\t') || line.starts_with("#\t"))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .map(PathBuf::from)
        .collect();

    Ok(conflicts)
}

/// Return a patch for unstaged changes under the absolute directory `path`.
///
/// File names are repository-relative by default, or relative to `path` when
/// `diff.relative=true`. Git's patch prefixes are retained.
#[instrument(level = "trace")]
pub(crate) async fn diff_worktree(path: &Path) -> Result<Vec<u8>, Error> {
    let output = git_cmd_for_dir()?
        .current_dir(path)
        .arg("diff")
        .hidden_args([
            "--full-index",
            "--no-ext-diff",
            "--no-textconv",
            "--ignore-submodules",
        ])
        .arg("--")
        .arg(path)
        // This diff is only used as a best-effort before/after snapshot of
        // hook changes. Some CI environments keep enough of `.git` for
        // `git ls-files` but omit blob objects needed by `git diff`; Git then
        // exits 128 on stderr with empty stdout. Keep comparing stdout in that
        // case so `run --all-files` can still run against the files Git can
        // enumerate.
        .check(false)
        .output()
        .await?;
    if !output.status.success() {
        debug!(
            status = %output.status,
            stderr = %String::from_utf8_lossy(&output.stderr),
            "Continuing with git diff stdout despite non-zero exit status"
        );
    }
    Ok(output.stdout)
}

/// Command that prints a patch for unstaged changes under `root`, for
/// `--show-diff-on-failure`, in the requested color mode.
pub(crate) fn diff_worktree_cmd(root: &Path, color: &str) -> Result<Cmd, Error> {
    let mut cmd = git_cmd_for_dir()?;
    cmd.current_dir(root)
        .arg("--no-pager")
        .arg("diff")
        .hidden_args(["--no-ext-diff"])
        .arg(color)
        .arg("--")
        .arg(root);
    Ok(cmd)
}

/// Create a tree object from the current index.
///
/// The name of the new tree object is printed to standard output.
/// The index must be in a fully merged state.
pub(crate) fn write_tree() -> Result<String, Error> {
    let output = git_cmd()?.arg("write-tree").check(true).output_sync()?;
    Ok(str::from_utf8(output.stdout.trim_ascii())?.to_string())
}

impl Repo {
    /// Discover repository directories from an absolute `cwd`, storing absolute paths.
    ///
    /// A Jujutsu workspace marks the repository backend as Jujutsu, and, when it has no
    /// `.git` entry at or below its root (a secondary workspace created with
    /// `jj workspace add`), points Git at the store resolved from its `.jj` metadata.
    /// The nearest boundary wins, so a Git repository nested inside a Jujutsu workspace
    /// is still treated as Git.
    #[instrument(level = "trace")]
    fn discover(cwd: &Path) -> Result<Self, Error> {
        let Some(jujutsu_root) = jj::find_workspace_root(cwd) else {
            return Self::discover_git(cwd, None);
        };
        let jujutsu_root = dunce::canonicalize(&jujutsu_root).unwrap_or(jujutsu_root);

        let plain = Self::discover_git(cwd, None);

        // A Git repository strictly inside the workspace is the nearer boundary, and it
        // does not depend on the Jujutsu metadata at all, so it is decided before the
        // store is read: broken metadata in an enclosing workspace must not hide a checkout
        // the user actually selected.
        if let Ok(repo) = &plain
            && repo
                .root
                .as_ref()
                .is_ok_and(|root| root.starts_with(&jujutsu_root) && root != &jujutsu_root)
        {
            return Self::discover_git(cwd, None);
        }

        let Some(store) = jj::resolve_git_store(cwd)? else {
            // The `.jj` metadata is unusable (a non-Git backend, or broken state). A Git
            // repository at or below the workspace still wins, but falling back to an
            // enclosing one would quietly hook a repository the user never selected.
            return match plain {
                Ok(repo)
                    if repo
                        .root
                        .as_ref()
                        .is_ok_and(|root| root.starts_with(&jujutsu_root)) =>
                {
                    Ok(repo)
                }
                Ok(_) | Err(Error::NotRepository) => {
                    Err(Error::JujutsuWithoutGitStore(jujutsu_root))
                }
                Err(err) => Err(err),
            };
        };

        match plain {
            Ok(mut repo) => {
                // A `.git` at the workspace boundary: a colocated workspace, or a secondary
                // one for which jj created a Git worktree. Git already resolves the right
                // directories, so only mark the backend.
                if repo
                    .root
                    .as_ref()
                    .is_ok_and(|root| root == &store.workspace_root)
                {
                    repo.jujutsu = true;
                    return Ok(repo);
                }
                // Git resolved a repository the caller named with `GIT_DIR`, and it is not this
                // workspace. Keep it rather than pointing Git at the workspace's backing store,
                // which would run the hooks against the wrong repository. A repository Git merely
                // walked up to (an enclosing checkout) does not get this treatment: the workspace
                // is the nearer boundary.
                if EnvVars.is_set(EnvVars::GIT_DIR) {
                    return Ok(repo);
                }
            }
            // Fall back to the backing store only when Git simply found no repository.
            // A repository that is broken, or one named by an inherited `GIT_DIR` that
            // does not resolve (`git` itself would have failed), is reported instead of
            // being masked by the store.
            Err(err)
                if EnvVars.is_set(EnvVars::GIT_DIR) || !matches!(err, Error::NotRepository) =>
            {
                return Err(err);
            }
            Err(_) => {}
        }

        let mut repo = Self::discover_git(cwd, Some(&store))?;
        repo.jujutsu = true;
        repo.backing_store = Some(store);
        Ok(repo)
    }

    /// Discover the Git directories by asking Git, optionally pointed at a Jujutsu
    /// workspace's backing store.
    #[instrument(level = "trace")]
    fn discover_git(cwd: &Path, jujutsu: Option<&jj::GitStore>) -> Result<Self, Error> {
        let rev_parse = |args: &[&str]| -> Result<_, Error> {
            // Use the unflagged builder: this call *is* what resolves a Jujutsu
            // workspace's backing store for the rest of the process.
            let mut cmd = raw_git_cmd()?;
            if let Some(store) = jujutsu {
                cmd.env(EnvVars::GIT_DIR, &store.git_dir);
                cmd.env(EnvVars::GIT_WORK_TREE, &store.workspace_root);
            }
            cmd.current_dir(cwd)
                .arg("rev-parse")
                .args(args)
                // Keep diagnostics stable so discovery errors can be identified.
                .env(EnvVars::LC_ALL, "C");
            let output = cmd.inner.as_std_mut().output()?;
            // Preserve errors from an invalid GIT_DIR or .git file.
            if !output.status.success()
                && output
                    .stderr
                    .split(|&b| b == b'\n')
                    .any(|line| line.starts_with(b"fatal: not a git repository (or any"))
            {
                return Err(Error::NotRepository);
            }
            Ok((cmd, output))
        };

        // Keep --show-toplevel last: bare repositories can resolve the Git paths
        // even though querying their working tree fails.
        // Keep Git's default path format for hooks. With core.hooksPath=,
        // --path-format=absolute fails instead of returning the current directory.
        let (cmd, output) = rev_parse(&[
            "--absolute-git-dir",
            "--git-common-dir",
            "--git-path",
            "hooks",
            "--show-toplevel",
        ])?;
        let output = if output.stdout.is_empty() {
            cmd.check_output(output)?
        } else {
            output
        };
        let stdout = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);

        let (git_dir, common_dir, hooks_dir, root) = if output.status.success()
            && let Some((git_dir, common_dir, hooks_dir, root)) =
                stdout.split(|&b| b == b'\n').collect_tuple()
        {
            (
                path_from_git_bytes(git_dir)?,
                path_from_git_bytes(common_dir)?,
                path_from_git_bytes(hooks_dir)?,
                Ok(path_from_git_bytes(root)?),
            )
        } else {
            // rev-parse cannot NUL-delimit these paths. Query them separately
            // for paths containing newlines or a repository without a worktree.
            let path = |args: &[&str]| -> Result<PathBuf, Error> {
                let (cmd, output) = rev_parse(args)?;
                let output = cmd.check_output(output)?;
                let stdout = output.stdout.strip_suffix(b"\n").unwrap_or(&output.stdout);
                path_from_git_bytes(stdout).map_err(Error::from)
            };
            (
                path(&["--absolute-git-dir"])?,
                path(&["--git-common-dir"])?,
                path(&["--git-path", "hooks"])?,
                path(&["--show-toplevel"]),
            )
        };
        let state = Self {
            root: root.map(|root| dunce::canonicalize(&root).unwrap_or(root)),
            git_dir,
            common_dir: cwd.join(common_dir),
            hooks_dir: cwd.join(hooks_dir),
            jujutsu: false,
            backing_store: None,
        };
        debug!(?state, "Git repository state");
        Ok(state)
    }
}

pub(crate) async fn init_repo(url: &str, path: &Path) -> Result<(), Error> {
    git_cmd()?
        // Unset `extensions.objectFormat` if set, just follow what hash the remote uses.
        .arg("-c")
        .arg("init.defaultObjectFormat=")
        .arg("init")
        .arg("--template=")
        .arg(path)
        .sanitize_git_repo_env()
        .check(true)
        .output()
        .await?;

    git_cmd()?
        .current_dir(path)
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(url)
        .sanitize_git_repo_env()
        .check(true)
        .output()
        .await?;

    Ok(())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalPrompt {
    Disabled,
    Enabled,
}

impl TerminalPrompt {
    fn env_value(self) -> &'static str {
        match self {
            Self::Disabled => "0",
            Self::Enabled => "1",
        }
    }
}

/// Return whether a git clone failure looks like an authentication error.
pub(crate) fn is_auth_error(err: &Error) -> bool {
    let Error::Command(process::Error::Status {
        error: StatusError {
            output: Some(output),
            ..
        },
        ..
    }) = err
    else {
        return false;
    };

    let error = String::from_utf8_lossy(&output.stderr).to_lowercase();

    [
        "terminal prompts disabled",
        "could not read username",
        "could not read password",
        "authentication failed",
        "http basic: access denied",
        "missing or invalid credentials",
        "could not authenticate to server",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

async fn shallow_clone(
    rev: &str,
    path: &Path,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    git_cmd()?
        .current_dir(path)
        .hidden_args(["-c", "protocol.version=2"])
        .arg("fetch")
        .arg("origin")
        .arg(rev)
        .arg("--depth=1")
        .sanitize_git_repo_env()
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;

    git_cmd()?
        .current_dir(path)
        .arg("checkout")
        .arg("FETCH_HEAD")
        .sanitize_git_repo_env()
        .env(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT, "1")
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;

    update_submodules(path, terminal_prompt, true).await?;

    Ok(())
}

async fn full_clone(rev: &str, path: &Path, terminal_prompt: TerminalPrompt) -> Result<(), Error> {
    git_cmd()?
        .current_dir(path)
        .arg("fetch")
        .arg("origin")
        .arg("--tags")
        .sanitize_git_repo_env()
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;

    git_cmd()?
        .current_dir(path)
        .arg("checkout")
        .arg(rev)
        .sanitize_git_repo_env()
        .env(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT, "1")
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;

    update_submodules(path, terminal_prompt, false).await?;

    Ok(())
}

async fn update_submodules(
    path: &Path,
    terminal_prompt: TerminalPrompt,
    shallow: bool,
) -> Result<(), Error> {
    if !should_update_submodules(path).await? {
        return Ok(());
    }

    let mut cmd = git_cmd()?;
    cmd.current_dir(path)
        .hidden_args(["-c", "protocol.version=2"]);
    cmd.arg("submodule")
        .arg("update")
        .arg("--init")
        .arg("--recursive");
    if shallow {
        cmd.arg("--depth=1");
    }
    cmd.sanitize_git_repo_env()
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;

    Ok(())
}

async fn should_update_submodules(path: &Path) -> Result<bool, Error> {
    if path.join(".gitmodules").try_exists()? {
        return Ok(true);
    }

    let output = git_cmd()?
        .current_dir(path)
        .arg("ls-files")
        .arg("-z")
        .arg("-s")
        .sanitize_git_repo_env()
        .env(EnvVars::LC_ALL, "C")
        .check(true)
        .output()
        .await?;

    Ok(output
        .stdout
        .split(|&byte| byte == b'\0')
        .any(|entry| entry.starts_with(b"160000 ")))
}

async fn clone_repo_attempt(
    rev: &str,
    path: &Path,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    if let Err(err) = shallow_clone(rev, path, terminal_prompt).await {
        if is_auth_error(&err) {
            warn!(?err, "Failed to shallow clone due to authentication error");
            return Err(err);
        }

        warn!(?err, "Failed to shallow clone, falling back to full clone");
        return full_clone(rev, path, terminal_prompt).await;
    }

    Ok(())
}

/// Clone a repository into an initialized destination with the requested terminal prompt mode.
pub(crate) async fn clone_repo(
    url: &str,
    rev: &str,
    path: &Path,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    init_repo(url, path).await?;
    clone_repo_attempt(rev, path, terminal_prompt).await
}

async fn config_value(scope: Option<&str>, key: &str) -> Result<Option<Vec<u8>>, Error> {
    let mut cmd = git_cmd()?;
    cmd.arg("config").arg("--includes");
    if let Some(scope) = scope {
        cmd.arg(scope);
    }
    let output = cmd
        .arg("--null")
        .arg("--get")
        .arg(key)
        .check(false)
        .output()
        .await?;
    Ok(output.status.success().then_some(output.stdout))
}

async fn has_config_value(scope: Option<&str>, key: &str) -> Result<bool, Error> {
    // An empty config value still counts as configured and can affect Git's
    // path resolution, e.g. `core.hooksPath=` makes `--git-path hooks`
    // resolve to the current directory.
    Ok(config_value(scope, key).await?.is_some())
}

async fn config_value_is_empty(scope: Option<&str>, key: &str) -> Result<bool, Error> {
    Ok(config_value(scope, key)
        .await?
        .as_deref()
        .is_some_and(|value| value.strip_suffix(b"\0").unwrap_or(value).is_empty()))
}

pub(crate) async fn has_hooks_path_set() -> Result<bool, Error> {
    has_config_value(None, "core.hooksPath").await
}

pub(crate) async fn has_repo_hooks_path_set() -> Result<bool, Error> {
    Ok(has_config_value(Some("--local"), "core.hooksPath").await?
        || has_config_value(Some("--worktree"), "core.hooksPath").await?)
}

/// Compute the file mode for a newly created file based on `core.sharedRepository`.
///
/// This mirrors the relevant parts of Git's `git_config_perm` in `setup.c`
/// and `calc_shared_perm` in `path.c`.
fn apply_shared_repository_file_mode(value: &str, mode: u32) -> Option<u32> {
    const PERM_GROUP: u32 = 0o660;
    const PERM_EVERYBODY: u32 = 0o664;

    fn apply(mode: u32, mut tweak: u32, replace: bool) -> u32 {
        // From Git's `calc_shared_perm`: if the original file is not
        // user-writable, do not introduce any write bits via the shared
        // repository permission tweak.
        if mode & 0o200 == 0 {
            tweak &= !0o222;
        }
        // Also from `calc_shared_perm`: for executable files, mirror read bits
        // into execute bits so an explicit mode like 0640 becomes 0750 when
        // applied to a 0755 file.
        if mode & 0o100 != 0 {
            tweak |= (tweak & 0o444) >> 2;
        }
        // Named values like `group` and `all` add permissions on top of the
        // existing mode, while octal values replace the low permission bits.
        if replace {
            (mode & !0o777) | tweak
        } else {
            mode | tweak
        }
    }

    let value = value.trim().to_ascii_lowercase();
    let (tweak, replace) = match value.as_str() {
        "" | "umask" | "false" | "no" | "off" | "0" => return None,
        "group" | "true" | "yes" | "on" | "1" => (PERM_GROUP, false),
        "all" | "world" | "everybody" | "2" => (PERM_EVERYBODY, false),
        // Parsed like Git's `git_config_perm`, which also accepts explicit
        // octal modes such as `0640`.
        _ => (u32::from_str_radix(&value, 8).ok()?, true),
    };

    // `git_config_perm` rejects explicit modes that do not grant user read/write.
    if replace && tweak & 0o600 != 0o600 {
        return None;
    }

    Some(apply(mode, tweak, replace))
}

/// Resolve the file mode implied by `core.sharedRepository` for a newly created file.
pub(crate) async fn shared_repository_file_mode(mode: u32) -> Result<u32> {
    let output = git_cmd()?
        .arg("config")
        .arg("--get")
        .arg("core.sharedRepository")
        .check(false)
        .output()
        .await?;
    if output.status.success() {
        let value = str::from_utf8(&output.stdout)?;
        Ok(apply_shared_repository_file_mode(value, mode).unwrap_or(mode))
    } else {
        Ok(mode)
    }
}

/// Return the input paths whose Git `filter` attribute is `lfs`.
///
/// Returned paths preserve their input spelling: relative paths are relative to
/// `current_dir`, and absolute paths stay absolute.
pub(crate) async fn lfs_files(
    current_dir: &Path,
    paths: &[&Path],
) -> Result<FxHashSet<PathBuf>, Error> {
    if paths.is_empty() {
        return Ok(FxHashSet::default());
    }

    let mut child = git_cmd()?
        .current_dir(current_dir)
        .arg("check-attr")
        .arg("filter")
        .arg("-z")
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .check(true)
        .spawn()?;

    let mut stdout = child.stdout.take().expect("failed to open stdout");
    let mut stdin = child.stdin.take().expect("failed to open stdin");

    let writer = async move {
        for path in paths {
            stdin.write_all(path_to_git_bytes(path)?).await?;
            stdin.write_all(b"\0").await?;
        }
        stdin.shutdown().await?;
        Ok::<(), std::io::Error>(())
    };
    let reader = async move {
        let mut out = Vec::new();
        stdout.read_to_end(&mut out).await?;
        Ok::<_, std::io::Error>(out)
    };

    let (read_result, _write_result) = tokio::try_join!(biased; reader, writer)?;

    let status = child.wait().await?;
    if !status.success() {
        return Err(Error::Command(process::Error::Status {
            command: "git check-attr -z filter --stdin".to_string(),
            error: StatusError {
                status,
                output: None,
            },
        }));
    }

    let mut lfs_files = FxHashSet::default();
    let mut it = read_result.split(|&byte| byte == b'\0');
    while let (Some(file), Some(_attr), Some(value)) = (it.next(), it.next(), it.next()) {
        if value == b"lfs" {
            lfs_files.insert(path_from_git_bytes(file)?);
        }
    }

    Ok(lfs_files)
}

/// Check if a git revision exists
pub(crate) async fn rev_exists(rev: &str) -> Result<bool, Error> {
    let output = git_cmd()?
        .arg("cat-file")
        // Exit with zero status if <object> exists and is a valid object.
        .arg("-e")
        .arg(rev)
        .check(false)
        .output()
        .await?;
    Ok(output.status.success())
}

/// Check if `ancestor` is an ancestor of `commit`.
pub(crate) async fn is_ancestor(ancestor: &str, commit: &str) -> Result<bool, Error> {
    let mut cmd = git_cmd()?;
    let status = cmd
        .arg("merge-base")
        .arg("--is-ancestor")
        .arg(ancestor)
        .arg(commit)
        .check(false)
        .status()
        .await?;

    if status.success() {
        return Ok(true);
    }
    if status.code() == Some(1) {
        return Ok(false);
    }

    cmd.check_status(status)?;
    Ok(false)
}

/// Return commits that are ancestors of the given commit but not in the specified remote.
pub(crate) async fn ancestors_not_in_remote(
    local_sha: &str,
    remote_name: &str,
) -> Result<Vec<String>, Error> {
    let output = git_cmd()?
        .arg("rev-list")
        .arg(local_sha)
        .arg("--topo-order")
        .arg("--reverse")
        .arg("--not")
        .arg(format!("--remotes={remote_name}"))
        .check(true)
        .output()
        .await?;
    Ok(str::from_utf8(&output.stdout)?
        .trim_ascii()
        .lines()
        .map(ToString::to_string)
        .collect())
}

/// Return root commits (commits with no parents) for the given commit.
pub(crate) async fn root_commits(local_sha: &str) -> Result<FxHashSet<String>, Error> {
    let output = git_cmd()?
        .arg("rev-list")
        .arg("--max-parents=0")
        .arg(local_sha)
        .check(true)
        .output()
        .await?;
    Ok(str::from_utf8(&output.stdout)?
        .trim_ascii()
        .lines()
        .map(ToString::to_string)
        .collect())
}

/// Return the parent commit of the given commit.
pub(crate) async fn parent_commit(commit: &str) -> Result<Option<String>, Error> {
    let output = git_cmd()?
        .arg("rev-parse")
        .arg(format!("{commit}^"))
        .check(false)
        .output()
        .await?;
    if output.status.success() {
        Ok(Some(
            str::from_utf8(&output.stdout)?.trim_ascii().to_string(),
        ))
    } else {
        Ok(None)
    }
}

/// Return absolute submodule paths by joining their configured paths to `git_root`.
/// `git_root` must be the absolute repository root.
#[instrument(level = "trace")]
pub(crate) fn list_submodules(git_root: &Path) -> Result<Vec<PathBuf>, Error> {
    if !git_root.join(".gitmodules").exists() {
        return Ok(vec![]);
    }

    let git = GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?;
    let mut cmd = Command::new(git);
    let output = apply_git_work_tree(&mut cmd)
        .current_dir(git_root)
        .arg("config")
        .arg("--null")
        .arg("--file")
        .arg(".gitmodules")
        .arg("--get-regexp")
        .arg(r"^submodule\..*\.path$")
        .output()?;

    let mut submodules = Vec::new();
    // With `--null`, Git separates each key from its value with `\n` and records with NUL.
    for entry in output.stdout.split(|&byte| byte == b'\0') {
        let Some(separator) = entry.iter().position(|&byte| byte == b'\n') else {
            continue;
        };
        let path = &entry[separator + 1..];
        if path.is_empty() {
            continue;
        }
        submodules.push(git_root.join(path_from_git_bytes(path)?));
    }
    Ok(submodules)
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    #[cfg(unix)]
    use crate::fs::PathClean;

    use super::zsplit;
    use super::{
        Error, GIT, TerminalPrompt, apply_shared_repository_file_mode, full_clone, init_repo,
        list_submodules, should_update_submodules, update_submodules,
    };
    #[cfg(unix)]
    use super::{Repo, lfs_files};
    use assert_cmd::assert::OutputAssertExt;

    fn run_git(path: &Path, args: &[&str]) {
        let mut command = Command::new(GIT.as_ref().unwrap());
        command.current_dir(path).args(args);

        command.assert().success();
    }

    #[tokio::test]
    async fn worktree_status_with_staged_and_unstaged_changes() -> anyhow::Result<()> {
        let tmp = tempfile::tempdir()?;
        let root = tmp.path();
        run_git(root, &["init"]);
        for name in ["staged.txt", "modified.txt", "deleted.txt"] {
            fs_err::write(root.join(name), "staged\n")?;
        }
        run_git(root, &["add", "."]);

        // A new repository has no HEAD but can still have a clean worktree.
        let status = super::worktree_status(root).await?;
        assert!(!status.unmerged);
        assert_eq!(status.unstaged, Vec::<PathBuf>::new());

        fs_err::write(root.join("modified.txt"), "unstaged\n")?;
        fs_err::remove_file(root.join("deleted.txt"))?;
        fs_err::write(root.join("intent.txt"), "")?;
        fs_err::write(root.join("untracked.txt"), "untracked\n")?;
        run_git(root, &["add", "--intent-to-add", "intent.txt"]);

        let status = super::worktree_status(root).await?;
        assert!(!status.unmerged);
        assert_eq!(
            status.unstaged,
            ["deleted.txt", "intent.txt", "modified.txt"].map(PathBuf::from)
        );
        assert_eq!(status.intent_to_add, [PathBuf::from("intent.txt")]);
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn worktree_status_preserves_unquoted_path_bytes() -> anyhow::Result<()> {
        use std::os::unix::ffi::OsStrExt as _;

        let status =
            super::parse_worktree_status(b"M\0 leading\nname-\xff.txt \0D\0deleted.txt\0")?;
        assert!(!status.unmerged);
        let paths = status
            .unstaged
            .iter()
            .map(|path| path.as_os_str().as_bytes())
            .collect::<Vec<_>>();
        assert_eq!(
            paths,
            [b" leading\nname-\xff.txt ".as_slice(), b"deleted.txt"]
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn repository_paths_preserve_special_characters() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let tmp = tempfile::tempdir().unwrap();
        let names: &[&[u8]] = &[
            b"repo\nwith newline\n",
            b"repo with spaces ",
            #[cfg(not(target_os = "macos"))]
            b"repo-\xff",
        ];
        for name in names {
            let repo = tmp.path().join(OsStr::from_bytes(name));
            let subdir = repo.join("subdir");
            fs_err::create_dir_all(&subdir).unwrap();
            run_git(&repo, &["init"]);

            let state = Repo::discover(&subdir).unwrap();
            assert_eq!(state.root.unwrap(), dunce::canonicalize(&repo).unwrap());
            assert_eq!(
                state.git_dir,
                dunce::canonicalize(repo.join(".git")).unwrap()
            );
            assert_eq!(state.common_dir.clean(), repo.join(".git"));
            assert_eq!(state.hooks_dir.clean(), repo.join(".git/hooks"));

            for hooks_dir in ["custom\nhooks ", " "] {
                run_git(&repo, &["config", "core.hooksPath", hooks_dir]);
                let state = Repo::discover(&subdir).unwrap();
                assert_eq!(state.hooks_dir.clean(), repo.join(hooks_dir));
            }

            let bare = repo.join("bare\n");
            run_git(&repo, &["init", "--bare", "bare\n"]);
            let state = Repo::discover(&bare).unwrap();
            assert!(state.root.is_err());
            assert_eq!(state.git_dir, dunce::canonicalize(&bare).unwrap());
            assert_eq!(state.common_dir.clean(), bare);
            assert_eq!(state.hooks_dir.clean(), bare.join("hooks"));
        }
    }

    #[tokio::test]
    async fn should_update_submodules_when_gitmodules_exists() {
        let tmp = tempfile::tempdir().unwrap();
        run_git(tmp.path(), &["init"]);
        fs_err::write(tmp.path().join(".gitmodules"), "").unwrap();

        assert!(should_update_submodules(tmp.path()).await.unwrap());
    }

    #[tokio::test]
    async fn full_clone_skips_submodule_update_when_repo_has_no_submodules() {
        let remote = tempfile::tempdir().unwrap();
        run_git(remote.path(), &["init"]);
        fs_err::write(remote.path().join("file.txt"), "content\n").unwrap();
        run_git(remote.path(), &["add", "."]);
        run_git(
            remote.path(),
            &[
                "-c",
                "user.name=prek",
                "-c",
                "user.email=prek@example.com",
                "commit",
                "-m",
                "initial commit",
            ],
        );
        let output = Command::new(GIT.as_ref().unwrap())
            .current_dir(remote.path())
            .arg("rev-parse")
            .arg("HEAD")
            .output()
            .unwrap();
        assert!(output.status.success());
        let rev = String::from_utf8_lossy(&output.stdout);

        let clone = tempfile::tempdir().unwrap();
        init_repo(remote.path().to_str().unwrap(), clone.path())
            .await
            .unwrap();

        full_clone(rev.trim(), clone.path(), TerminalPrompt::Disabled)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn update_submodules_runs_when_gitlinks_exist_without_gitmodules() {
        let tmp = tempfile::tempdir().unwrap();
        run_git(tmp.path(), &["init"]);
        run_git(
            tmp.path(),
            &[
                "update-index",
                "--add",
                "--cacheinfo",
                "160000,1111111111111111111111111111111111111111,sub",
            ],
        );

        assert!(should_update_submodules(tmp.path()).await.unwrap());

        let err = update_submodules(tmp.path(), TerminalPrompt::Disabled, true)
            .await
            .unwrap_err();

        assert_matches!(err, Error::Command(_));
        let message = err.to_string();
        assert!(message.contains("submodule update --init --recursive"));
        assert!(message.contains("--depth=1"));

        let err = update_submodules(tmp.path(), TerminalPrompt::Disabled, false)
            .await
            .unwrap_err();

        assert_matches!(err, Error::Command(_));
        let message = err.to_string();
        assert!(message.contains("submodule update --init --recursive"));
        assert!(!message.contains("--depth=1"));
    }

    #[cfg(unix)]
    #[test]
    fn zsplit_preserves_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt as _;

        let paths = zsplit(b"normal.py\0bad-\xff.py\0").unwrap();

        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].as_os_str().as_bytes(), b"normal.py");
        assert_eq!(paths[1].as_os_str().as_bytes(), b"bad-\xff.py");
    }

    #[test]
    fn zsplit_preserves_leading_and_trailing_spaces() {
        let paths = zsplit(b" leading.py\0trailing.py \0").unwrap();

        assert_eq!(
            paths,
            vec![PathBuf::from(" leading.py"), PathBuf::from("trailing.py ")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn diff_files_preserve_unusual_paths() -> anyhow::Result<()> {
        use std::os::unix::ffi::OsStrExt as _;

        let files = super::parse_diff_files(
            b":000000 100644 000000 abc123 A\0added.rs\0\
              :100644 000000 abc123 000000 D\0 leading\t\n\xff.rs\0\
              :100644 100644 abc123 def456 M\0modified.rs\0\
              :120000 000000 def456 000000 D\0link with spaces.rs\0",
        )?;

        assert_eq!(files.len(), 4);
        assert_eq!(files[0].path, Path::new("added.rs"));
        assert!(files[0].deleted_mode.is_none());
        assert_eq!(files[1].path.as_os_str().as_bytes(), b" leading\t\n\xff.rs");
        assert!(matches!(
            files[1].deleted_mode,
            Some(super::FileMode::Regular)
        ));
        assert_eq!(files[2].path, Path::new("modified.rs"));
        assert!(files[2].deleted_mode.is_none());
        assert_eq!(files[3].path, Path::new("link with spaces.rs"));
        assert!(matches!(
            files[3].deleted_mode,
            Some(super::FileMode::Symlink)
        ));
        Ok(())
    }

    #[test]
    fn list_submodules_preserves_spaces_in_paths() {
        let tmp = tempfile::tempdir().unwrap();
        run_git(tmp.path(), &["init"]);
        fs_err::write(
            tmp.path().join(".gitmodules"),
            "[submodule \"space\"]\n\tpath = modules/with space\n",
        )
        .unwrap();

        assert_eq!(
            list_submodules(tmp.path()).unwrap(),
            vec![tmp.path().join("modules/with space")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn list_submodules_preserves_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt as _;

        let tmp = tempfile::tempdir().unwrap();
        run_git(tmp.path(), &["init"]);
        fs_err::write(
            tmp.path().join(".gitmodules"),
            b"[submodule \"raw\"]\n\tpath = modules/bad-\xff\n",
        )
        .unwrap();

        let submodules = list_submodules(tmp.path()).unwrap();

        assert_eq!(
            submodules[0]
                .strip_prefix(tmp.path())
                .unwrap()
                .as_os_str()
                .as_bytes(),
            b"modules/bad-\xff"
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn lfs_files_preserves_non_utf8_paths() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let tmp = tempfile::tempdir().unwrap();
        run_git(tmp.path(), &["init"]);
        fs_err::write(tmp.path().join(".gitattributes"), "* filter=lfs\n").unwrap();
        let path = Path::new(OsStr::from_bytes(b"bad-\xff.bin"));

        let files = lfs_files(tmp.path(), &[path]).await.unwrap();

        assert!(files.contains(path));
    }

    #[test]
    fn shared_repository_group_mode_matches_git_behavior() {
        for value in ["group", "true", "yes", "on", "1"] {
            assert_eq!(apply_shared_repository_file_mode(value, 0o755), Some(0o775));
        }
    }

    #[test]
    fn shared_repository_everybody_mode_matches_git_behavior() {
        for value in ["all", "world", "everybody", "2"] {
            assert_eq!(apply_shared_repository_file_mode(value, 0o755), Some(0o775));
        }
    }

    #[test]
    fn shared_repository_octal_mode_matches_git_behavior() {
        assert_eq!(
            apply_shared_repository_file_mode("0640", 0o644),
            Some(0o640)
        );
        assert_eq!(
            apply_shared_repository_file_mode("0640", 0o755),
            Some(0o750)
        );
    }

    #[test]
    fn shared_repository_umask_or_invalid_values_do_not_override_mode() {
        for value in ["", "umask", "false", "no", "off", "0", "invalid", "0400"] {
            assert_eq!(apply_shared_repository_file_mode(value, 0o755), None);
        }
    }
}
