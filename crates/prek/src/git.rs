use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::Utf8Error;
use std::sync::{LazyLock, OnceLock};

use anyhow::Result;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use rustc_hash::FxHashSet;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::{debug, instrument, warn};

use crate::fs::PathClean;
use crate::process;
use crate::process::{Cmd, StatusError};

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Command(#[from] process::Error),

    #[error("Failed to find git: {0}")]
    GitNotFound(#[from] which::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    UTF8(#[from] Utf8Error),

    #[error(
        "Git resolved hooks directory to the current directory (`{0}`). Unset `core.hooksPath` or set it to a real directory path."
    )]
    InvalidHooksPath(PathBuf),
}

pub(crate) static GIT: LazyLock<Result<PathBuf, which::Error>> =
    LazyLock::new(|| which::which("git"));

// Git hooks can expose `GIT_DIR` without `GIT_WORK_TREE`. Keep the derived
// work tree scoped to prek's own git commands so user hooks do not inherit it.
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

fn git_work_tree() -> Option<&'static Path> {
    GIT_WORK_TREE.get().and_then(Option::as_deref)
}

pub(crate) static GIT_ROOT: LazyLock<Result<PathBuf, Error>> = LazyLock::new(|| {
    root()
        .map(|root| dunce::canonicalize(&root).unwrap_or(root))
        .inspect(|root| {
            debug!("Git root: {}", root.display());
        })
});

/// Repository-local environment variables cleared before operating on another repository.
///
/// `GIT_CONFIG_PARAMETERS`, `GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_*`, and `GIT_CONFIG_VALUE_*`
/// are deliberately excluded so nested Git commands retain caller-supplied command-scoped settings.
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
    fn sanitize_git_repo_env(&mut self) -> &mut Self;
}

pub(crate) fn apply_git_work_tree(cmd: &mut Command) -> &mut Command {
    if let Some(work_tree) = git_work_tree() {
        cmd.env(EnvVars::GIT_WORK_TREE, work_tree);
    }
    cmd
}

impl GitCommandExt for Cmd {
    fn sanitize_git_repo_env(&mut self) -> &mut Self {
        for key in GIT_REPO_LOCAL_ENVS {
            self.env_remove(key);
        }
        self
    }
}

pub(crate) fn git_cmd() -> Result<Cmd, Error> {
    let mut cmd = Cmd::new(GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?);
    cmd.hidden_args(["-c", "core.useBuiltinFSMonitor=false"]);
    if let Some(work_tree) = git_work_tree() {
        cmd.env(EnvVars::GIT_WORK_TREE, work_tree);
    }

    Ok(cmd)
}

fn zsplit(s: &[u8]) -> Result<Vec<PathBuf>, Utf8Error> {
    s.split(|&b| b == b'\0')
        .filter(|slice| !slice.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

#[cfg(unix)]
#[expect(clippy::unnecessary_wraps)]
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf, Utf8Error> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf, Utf8Error> {
    str::from_utf8(bytes).map(PathBuf::from)
}

#[cfg(unix)]
#[expect(clippy::unnecessary_wraps)]
fn path_to_git_bytes(path: &Path) -> std::io::Result<&[u8]> {
    use std::os::unix::ffi::OsStrExt as _;

    Ok(path.as_os_str().as_bytes())
}

#[cfg(not(unix))]
fn path_to_git_bytes(path: &Path) -> std::io::Result<&[u8]> {
    path.to_str().map(str::as_bytes).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("Path is not valid UTF-8: `{}`", path.display()),
        )
    })
}

pub(crate) async fn intent_to_add_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = git_cmd()?
        .arg("diff")
        .hidden_args(["--no-ext-diff", "--ignore-submodules"])
        .arg("--diff-filter=A")
        .arg("--name-only")
        .arg("-z")
        .arg("--")
        .arg(root)
        .check(true)
        .output()
        .await?;
    Ok(zsplit(&output.stdout)?)
}

pub(crate) async fn staged_added_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = git_cmd()?
        .current_dir(root)
        .arg("diff")
        .arg("--staged")
        // `git diff --name-only` reports paths relative to the repository root by default,
        // even when it runs inside a subdirectory. `--relative` keeps the output aligned
        // with hooks, which receive filenames relative to their project root.
        .arg("--relative")
        .arg("--name-only")
        .arg("--diff-filter=A")
        .arg("-z") // Use NUL as line terminator
        .check(true)
        .output()
        .await?;
    Ok(zsplit(&output.stdout)?)
}

pub(crate) async fn changed_files(
    old: &str,
    new: &str,
    root: &Path,
) -> Result<Vec<PathBuf>, Error> {
    let build_cmd = |range: String| -> Result<Cmd, Error> {
        let mut cmd = git_cmd()?;
        cmd.arg("diff")
            .arg("--name-only")
            .arg("--diff-filter=ACMRT")
            .hidden_args(["--no-ext-diff"])
            .arg("-z") // Use NUL as line terminator
            .arg(range)
            .arg("--")
            .arg(root);
        Ok(cmd)
    };

    // Try three-dot syntax first (merge-base diff), which works for commits
    let output = build_cmd(format!("{old}...{new}"))?
        .check(false)
        .output()
        .await?;

    if output.status.success() {
        return Ok(zsplit(&output.stdout)?);
    }

    // Fall back to two-dot syntax, which works with both commits and trees
    let output = build_cmd(format!("{old}..{new}"))?
        .check(true)
        .output()
        .await?;
    Ok(zsplit(&output.stdout)?)
}

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

pub(crate) async fn git_dir() -> Result<PathBuf, Error> {
    let output = git_cmd()?
        .arg("rev-parse")
        .arg("--git-dir")
        .check(true)
        .output()
        .await?;
    path_from_git_bytes(output.stdout.trim_ascii()).map_err(Error::from)
}

pub(crate) async fn common_dir() -> Result<PathBuf, Error> {
    let output = git_cmd()?
        .arg("rev-parse")
        .arg("--git-common-dir")
        .check(true)
        .output()
        .await?;
    path_from_git_bytes(output.stdout.trim_ascii()).map_err(Error::from)
}

pub(crate) async fn hooks_dir() -> Result<PathBuf, Error> {
    // Ask Git for the effective hooks directory instead of reconstructing it
    // ourselves. That lets Git apply the full precedence chain for
    // `core.hooksPath`, including local/worktree config, linked worktrees, bare
    // + worktree layouts, and repo-owned config loaded through `include.path`
    // / `includeIf`.
    let output = git_cmd()?
        .arg("rev-parse")
        .arg("--git-path")
        .arg("hooks")
        .check(true)
        .output()
        .await?;
    let stdout = output.stdout.trim_ascii();
    let hooks_dir = if stdout.is_empty() {
        common_dir().await?.join("hooks")
    } else {
        path_from_git_bytes(stdout)?
    };

    let cleaned = hooks_dir.clean();
    // `core.hooksPath=` is a particularly dangerous case: Git treats it as
    // configured, but resolves `--git-path hooks` to the current directory. If
    // we accepted that value, install/uninstall would write or remove hook
    // shims from the worktree root. Keep the explicit `core.hooksPath=.` case
    // working, but reject the empty-string variant.
    if cleaned == Path::new(".") && config_value_is_empty(None, "core.hooksPath").await? {
        Err(Error::InvalidHooksPath(cleaned))
    } else {
        Ok(hooks_dir)
    }
}

pub(crate) async fn staged_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = git_cmd()?
        .current_dir(root)
        .arg("diff")
        .arg("--cached")
        .arg("--name-only")
        .arg("--diff-filter=ACMRTUXB") // Everything except for D
        .hidden_args(["--no-ext-diff"])
        .arg("-z") // Use NUL as line terminator
        .check(true)
        .output()
        .await?;
    Ok(zsplit(&output.stdout)?)
}

pub(crate) async fn files_not_staged(files: &[&Path]) -> Result<Vec<PathBuf>> {
    let output = git_cmd()?
        .arg("diff")
        .arg("--exit-code")
        .arg("--name-only")
        .hidden_args(["--no-ext-diff"])
        .arg("-z") // Use NUL as line terminator
        .file_args(files)
        .check(false)
        .output()
        .await?;

    if output.status.code().is_some_and(|code| code == 1) {
        return Ok(zsplit(&output.stdout)?);
    }

    Ok(vec![])
}

pub(crate) async fn has_unmerged_paths() -> Result<bool, Error> {
    let output = git_cmd()?
        .arg("ls-files")
        .arg("--unmerged")
        .check(true)
        .output()
        .await?;
    Ok(!output.stdout.trim_ascii().is_empty())
}

pub(crate) async fn has_diff(rev: &str, path: &Path) -> Result<bool> {
    let status = git_cmd()?
        .arg("diff")
        .arg("--quiet")
        .arg(rev)
        .current_dir(path)
        .check(false)
        .status()
        .await?;
    Ok(status.code() == Some(1))
}

pub(crate) async fn is_in_merge_conflict() -> Result<bool, Error> {
    let git_dir = git_dir().await?;
    Ok(git_dir.join("MERGE_HEAD").try_exists()? && git_dir.join("MERGE_MSG").try_exists()?)
}

pub(crate) async fn conflicted_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let tree = git_cmd()?.arg("write-tree").check(true).output().await?;

    let output = git_cmd()?
        .arg("diff")
        .arg("--name-only")
        .hidden_args(["--no-ext-diff"])
        .arg("-z") // Use NUL as line terminator
        .arg("-m") // Show diffs for merge commits in the default format.
        .arg(String::from_utf8_lossy(&tree.stdout).trim_ascii())
        .arg("HEAD")
        .arg("MERGE_HEAD")
        .arg("--")
        .arg(root)
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

async fn parse_merge_msg_for_conflicts() -> Result<Vec<PathBuf>, Error> {
    let git_dir = git_dir().await?;
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

#[instrument(level = "trace")]
pub(crate) async fn has_worktree_diff(path: &Path) -> Result<bool, Error> {
    let mut cmd = git_cmd()?;
    let status = cmd
        .arg("diff-files")
        .arg("--quiet")
        .hidden_args(["--no-ext-diff", "--no-textconv", "--ignore-submodules"])
        .arg("--")
        .arg(path)
        .check(false)
        .status()
        .await?;

    if status.success() {
        return Ok(false);
    }
    if status.code() == Some(1) {
        return Ok(true);
    }

    cmd.check_status(status)?;
    Ok(true)
}

#[instrument(level = "trace")]
pub(crate) async fn diff_worktree(path: &Path) -> Result<Vec<u8>, Error> {
    let output = git_cmd()?
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

/// Create a tree object from the current index.
///
/// The name of the new tree object is printed to standard output.
/// The index must be in a fully merged state.
pub(crate) async fn write_tree() -> Result<String, Error> {
    let output = git_cmd()?.arg("write-tree").check(true).output().await?;
    Ok(str::from_utf8(output.stdout.trim_ascii())?.to_string())
}

/// Return the path of the top-level directory of the working tree.
#[instrument(level = "trace")]
pub(crate) fn root() -> Result<PathBuf, Error> {
    let git = GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?;
    let mut cmd = Command::new(git);
    let output = apply_git_work_tree(&mut cmd)
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()?;
    if !output.status.success() {
        return Err(Error::Command(process::Error::Status {
            command: format!("{} rev-parse --show-toplevel", git.to_string_lossy()),
            error: StatusError {
                status: output.status,
                output: Some(output),
            },
        }));
    }

    path_from_git_bytes(output.stdout.trim_ascii()).map_err(Error::from)
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

/// Return a list of absolute paths of all git submodules in the repository.
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
    #[cfg(unix)]
    use super::lfs_files;
    use super::zsplit;
    use super::{
        Error, GIT, TerminalPrompt, apply_shared_repository_file_mode, full_clone, init_repo,
        list_submodules, should_update_submodules, update_submodules,
    };
    use assert_cmd::assert::OutputAssertExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn run_git(path: &Path, args: &[&str]) {
        let mut command = Command::new(GIT.as_ref().unwrap());
        command.current_dir(path).args(args);

        command.assert().success();
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

        assert!(matches!(err, Error::Command(_)));
        let message = err.to_string();
        assert!(message.contains("submodule update --init --recursive"));
        assert!(message.contains("--depth=1"));

        let err = update_submodules(tmp.path(), TerminalPrompt::Disabled, false)
            .await
            .unwrap_err();

        assert!(matches!(err, Error::Command(_)));
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
