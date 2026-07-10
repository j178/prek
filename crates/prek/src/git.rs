use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str::Utf8Error;
use std::sync::LazyLock;

use anyhow::Result;
use prek_consts::env_vars::EnvVars;
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

pub(crate) static GIT_ROOT: LazyLock<Result<PathBuf, Error>> = LazyLock::new(|| {
    get_root()
        .map(|root| dunce::canonicalize(&root).unwrap_or(root))
        .inspect(|root| {
            debug!("Git root: {}", root.display());
        })
});

/// Remove some `GIT_` environment variables exposed by `git`.
///
/// For some commands, like `git commit -a` or `git commit -p`, git creates a `.git/index.lock` file
/// and set `GIT_INDEX_FILE` to point to it.
/// We need to keep the `GIT_INDEX_FILE` env var to make sure `git write-tree` works correctly.
/// <https://stackoverflow.com/questions/65639403/git-pre-commit-hook-how-can-i-get-added-modified-files-when-commit-with-a-flag/65647202#65647202>
pub(crate) static GIT_ENV_TO_REMOVE: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
    let keep = &[
        "GIT_EXEC_PATH",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
        "GIT_SSL_CAINFO",
        "GIT_SSL_NO_VERIFY",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_HTTP_PROXY_AUTHMETHOD",
        "GIT_ALLOW_PROTOCOL",
        "GIT_ASKPASS",
    ];

    std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("GIT_")
                && !k.starts_with("GIT_CONFIG_KEY_")
                && !k.starts_with("GIT_CONFIG_VALUE_")
                && !keep.contains(&k.as_str())
        })
        .collect()
});

pub(crate) fn git_cmd() -> Result<Cmd, Error> {
    let mut cmd = Cmd::new(GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?);
    cmd.hidden_args(["-c", "core.useBuiltinFSMonitor=false"]);

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

pub(crate) async fn get_added_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
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

pub(crate) async fn get_changed_files(
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

#[instrument(level = "trace")]
pub(crate) async fn ls_files(cwd: &Path, path: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = git_cmd()?
        .current_dir(cwd)
        .arg("ls-files")
        .arg("-z")
        .arg("--")
        .arg(path)
        .check(true)
        .output()
        .await?;

    Ok(zsplit(&output.stdout)?)
}

pub(crate) async fn get_git_dir() -> Result<PathBuf, Error> {
    let output = git_cmd()?
        .arg("rev-parse")
        .arg("--git-dir")
        .check(true)
        .output()
        .await?;
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim_ascii(),
    ))
}

pub(crate) async fn get_git_common_dir() -> Result<PathBuf, Error> {
    let output = git_cmd()?
        .arg("rev-parse")
        .arg("--git-common-dir")
        .check(true)
        .output()
        .await?;
    if output.stdout.trim_ascii().is_empty() {
        Ok(get_git_dir().await?)
    } else {
        Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim_ascii(),
        ))
    }
}

pub(crate) async fn get_git_hooks_dir() -> Result<PathBuf, Error> {
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
    let hooks_dir = if output.stdout.trim_ascii().is_empty() {
        get_git_common_dir().await?.join("hooks")
    } else {
        PathBuf::from(String::from_utf8_lossy(&output.stdout).trim_ascii())
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

pub(crate) async fn get_staged_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
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
    let git_dir = get_git_dir().await?;
    Ok(git_dir.join("MERGE_HEAD").try_exists()? && git_dir.join("MERGE_MSG").try_exists()?)
}

pub(crate) async fn get_conflicted_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
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
    let git_dir = get_git_dir().await?;
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
pub(crate) async fn get_diff(path: &Path) -> Result<Vec<u8>, Error> {
    let output = git_cmd()?
        .arg("diff")
        .hidden_args(["--no-ext-diff", "--no-textconv", "--ignore-submodules"])
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
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_ascii()
        .to_string())
}

/// Get the path of the top-level directory of the working tree.
#[instrument(level = "trace")]
pub(crate) fn get_root() -> Result<PathBuf, Error> {
    let git = GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?;
    let output = std::process::Command::new(git)
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

/// Ensure a shared bare source repository exists and points at its upstream.
pub(crate) async fn ensure_bare_repo(repo: &str, source: &Path) -> Result<(), Error> {
    let valid = if source.join("HEAD").try_exists()? {
        let output = git_cmd()?
            .arg("--git-dir")
            .arg(source)
            .arg("rev-parse")
            .arg("--is-bare-repository")
            .remove_git_envs()
            .check(false)
            .output()
            .await?;
        output.status.success() && output.stdout.trim_ascii() == b"true"
    } else {
        false
    };

    if !valid {
        if source.try_exists()? {
            let metadata = fs_err::tokio::symlink_metadata(source).await?;
            if metadata.is_dir() {
                fs_err::tokio::remove_dir_all(source).await?;
            } else {
                fs_err::tokio::remove_file(source).await?;
            }
        }
        if let Some(parent) = source.parent() {
            fs_err::tokio::create_dir_all(parent).await?;
        }

        git_cmd()?
            // Unset `extensions.objectFormat` if set, just follow what hash the remote uses.
            .arg("-c")
            .arg("init.defaultObjectFormat=")
            .arg("init")
            .arg("--bare")
            .arg("--template=")
            .arg(source)
            .remove_git_envs()
            .check(true)
            .output()
            .await?;

        git_cmd()?
            .arg("--git-dir")
            .arg(source)
            .arg("remote")
            .arg("add")
            .arg("origin")
            .arg(repo)
            .remove_git_envs()
            .check(true)
            .output()
            .await?;

        return Ok(());
    }

    let output = git_cmd()?
        .arg("--git-dir")
        .arg(source)
        .arg("remote")
        .arg("set-url")
        .arg("origin")
        .arg(repo)
        .remove_git_envs()
        .check(false)
        .output()
        .await?;
    if !output.status.success() {
        git_cmd()?
            .arg("--git-dir")
            .arg(source)
            .arg("remote")
            .arg("add")
            .arg("origin")
            .arg(repo)
            .remove_git_envs()
            .check(true)
            .output()
            .await?;
    }

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

fn is_full_object_id(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

async fn try_resolve_repo_source_commit(source: &Path, rev: &str) -> Result<Option<String>, Error> {
    let output = git_cmd()?
        .arg("--git-dir")
        .arg(source)
        .arg("rev-parse")
        .arg(format!("{rev}^{{commit}}"))
        .remove_git_envs()
        .check(false)
        .output()
        .await?;
    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout)
                .trim_ascii()
                .to_string(),
        ))
    } else {
        Ok(None)
    }
}

async fn resolve_repo_source_commit(source: &Path, rev: &str) -> Result<String, Error> {
    if let Some(commit) = try_resolve_repo_source_commit(source, rev).await? {
        return Ok(commit);
    }

    let remote_rev = format!("refs/remotes/origin/{rev}");
    if let Some(commit) = try_resolve_repo_source_commit(source, &remote_rev).await? {
        return Ok(commit);
    }

    let output = git_cmd()?
        .arg("--git-dir")
        .arg(source)
        .arg("rev-parse")
        .arg(format!("{rev}^{{commit}}"))
        .remove_git_envs()
        .check(true)
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_ascii()
        .to_string())
}

async fn pin_repo_source_commit(source: &Path, commit: &str) -> Result<(), Error> {
    git_cmd()?
        .arg("--git-dir")
        .arg(source)
        .arg("update-ref")
        .arg(repo_source_pin(commit))
        .arg(commit)
        .remove_git_envs()
        .check(true)
        .output()
        .await?;
    Ok(())
}

fn repo_source_pin(commit: &str) -> String {
    format!("refs/heads/prek/{commit}")
}

async fn repo_source_commit_is_pinned(source: &Path, commit: &str) -> Result<bool, Error> {
    Ok(git_cmd()?
        .arg("--git-dir")
        .arg(source)
        .args(["show-ref", "--verify", "--quiet"])
        .arg(repo_source_pin(commit))
        .remove_git_envs()
        .check(false)
        .output()
        .await?
        .status
        .success())
}

async fn repo_source_is_partial(source: &Path) -> Result<bool, Error> {
    let output = git_cmd()?
        .arg("--git-dir")
        .arg(source)
        .args(["config", "--bool", "--get", "remote.origin.promisor"])
        .remove_git_envs()
        .check(false)
        .output()
        .await?;
    Ok(output.status.success() && output.stdout.trim_ascii() == b"true")
}

async fn fetch_repo_source_shallow(
    source: &Path,
    rev: &str,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    git_cmd()?
        .hidden_args(["-c", "protocol.version=2"])
        .arg("--git-dir")
        .arg(source)
        .arg("fetch")
        .arg("origin")
        .arg(rev)
        .arg("--depth=1")
        .remove_git_envs()
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;
    Ok(())
}

async fn materialize_repo_source_revision(
    source: &Path,
    rev: &str,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    // Walk the snapshot's tree, rather than the commit, so history is excluded. Fetching all
    // promised objects reported by `--missing=print` in one request avoids both one-fetch-per-blob
    // lazy loading and the shallow-boundary changes caused by another depth-one commit fetch.
    let output = git_cmd()?
        .arg("--git-dir")
        .arg(source)
        .args([
            "rev-list",
            "--objects",
            "--missing=print",
            "--no-object-names",
        ])
        .arg(format!("{rev}^{{tree}}"))
        .remove_git_envs()
        .check(true)
        .output()
        .await?;
    let missing = str::from_utf8(&output.stdout)?
        .lines()
        .filter_map(|line| line.strip_prefix('?'))
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(());
    }

    let mut fetch = git_cmd()?;
    fetch
        .hidden_args(["-c", "fetch.negotiationAlgorithm=noop"])
        .arg("--git-dir")
        .arg(source)
        .arg("fetch")
        .arg("origin")
        .args([
            "--no-tags",
            "--no-write-fetch-head",
            "--recurse-submodules=no",
            "--stdin",
        ])
        .remove_git_envs()
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    let mut child = fetch.spawn()?;
    let mut stdin = child.stdin.take().expect("failed to open stdin");
    for object in missing {
        stdin.write_all(object.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
    }
    stdin.shutdown().await?;
    drop(stdin);
    fetch.check_status(child.wait().await?)?;
    Ok(())
}

async fn fetch_repo_source_full(
    source: &Path,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    let is_shallow = source.join("shallow").try_exists()?;
    let mut cmd = git_cmd()?;
    cmd.hidden_args(["--git-dir"])
        .hidden_args([source.as_os_str()])
        .arg("fetch")
        .arg("origin")
        .hidden_args([
            "+refs/heads/*:refs/remotes/origin/*",
            "+refs/tags/*:refs/tags/*",
            "--prune",
        ]);
    if is_shallow {
        cmd.hidden_args(["--unshallow"]);
    } else {
        cmd.hidden_args(["--update-shallow"]);
    }
    cmd.remove_git_envs()
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;
    Ok(())
}

/// Fetch a revision into a shared bare source and return its exact commit.
///
/// A source has two object-completeness levels: refs fetched by `prek update` have a complete
/// commit graph but may omit blobs, while `refs/heads/prek/<commit>` certifies that the commit's
/// depth-one tree and blobs are materialized for a derived checkout. Object existence alone is
/// therefore not a checkout-readiness check once the source becomes a partial repository.
pub(crate) async fn fetch_repo_source_revision(
    source: &Path,
    rev: &str,
    terminal_prompt: TerminalPrompt,
) -> Result<String, Error> {
    if is_full_object_id(rev)
        && let Some(commit) = try_resolve_repo_source_commit(source, rev).await?
    {
        if repo_source_commit_is_pinned(source, &commit).await? {
            return Ok(commit);
        }
        if repo_source_is_partial(source).await? {
            materialize_repo_source_revision(source, &commit, terminal_prompt).await?;
        }
        pin_repo_source_commit(source, &commit).await?;
        return Ok(commit);
    }

    let commit = match fetch_repo_source_shallow(source, rev, terminal_prompt).await {
        Ok(()) => {
            let commit = resolve_repo_source_commit(source, "FETCH_HEAD").await?;
            if repo_source_is_partial(source).await? {
                materialize_repo_source_revision(source, &commit, terminal_prompt).await?;
            }
            commit
        }
        Err(err) => {
            if is_auth_error(&err) {
                warn!(
                    ?err,
                    "Failed to fetch repo source due to authentication error"
                );
                return Err(err);
            }

            warn!(
                ?err,
                "Failed to fetch repo source revision, falling back to full fetch"
            );
            fetch_repo_source_full(source, terminal_prompt).await?;
            let commit = resolve_repo_source_commit(source, rev).await?;
            if repo_source_is_partial(source).await? {
                materialize_repo_source_revision(source, &commit, terminal_prompt).await?;
            }
            commit
        }
    };

    // Create the pin only after the revision is complete; checkout treats this ref as its readiness
    // marker as well as the branch exposed to the depth-one transport clone.
    pin_repo_source_commit(source, &commit).await?;
    Ok(commit)
}

/// Refresh the default branch and tags used by `prek update`.
///
/// Update needs the complete commit graph for ancestry checks, but not historical file contents.
/// A blobless fetch can coexist with complete pinned snapshots already added by normal runs. If a
/// run added a depth-one boundary, `--unshallow` repairs the graph before update inspects ancestry.
pub(crate) async fn fetch_repo_source_for_update(source: &Path) -> Result<(), Error> {
    let is_shallow = source.join("shallow").try_exists()?;
    let mut cmd = git_cmd()?;
    cmd.hidden_args(["-c", "protocol.version=2"])
        .arg("--git-dir")
        .arg(source)
        .arg("fetch")
        .arg("--filter=blob:none")
        .arg("origin")
        .arg("HEAD")
        .arg("+refs/tags/*:refs/tags/*")
        .arg("--prune");
    if is_shallow {
        cmd.arg("--unshallow");
    } else {
        cmd.arg("--update-shallow");
    }
    cmd.remove_git_envs()
        .env(EnvVars::LC_ALL, "C")
        .check(true)
        .output()
        .await?;
    Ok(())
}

/// Create an independent checkout from a shared bare source.
pub(crate) async fn checkout_repo_from_source(
    source: &Path,
    repo: &str,
    revision: &str,
    target: &Path,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    git_cmd()?
        .arg("clone")
        .arg("--no-checkout")
        // Only the pin's depth-one closure is guaranteed complete. Force the transport path so Git
        // honors `--depth=1` for a local source; a normal local clone ignores depth and could copy
        // blobless history from the update view into a non-promisor checkout.
        .arg("--no-local")
        .arg("--depth=1")
        .arg("--single-branch")
        .arg("--no-tags")
        .arg("--branch")
        .arg(format!("prek/{revision}"))
        .arg("--template=")
        .arg("--origin=origin")
        .arg(source)
        .arg(target)
        .remove_git_envs()
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;

    git_cmd()?
        .current_dir(target)
        .arg("remote")
        .arg("set-url")
        .arg("origin")
        .arg(repo)
        .remove_git_envs()
        .check(true)
        .output()
        .await?;

    git_cmd()?
        .current_dir(target)
        .arg("checkout")
        .arg("--quiet")
        .arg(revision)
        .remove_git_envs()
        .env(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT, "1")
        .env(EnvVars::LC_ALL, "C")
        .env(EnvVars::GIT_TERMINAL_PROMPT, terminal_prompt.env_value())
        .check(true)
        .output()
        .await?;

    if let Err(err) = update_submodules(target, terminal_prompt, true).await {
        if is_auth_error(&err) {
            return Err(err);
        }
        warn!(
            ?err,
            "Failed to shallow clone submodules, falling back to full clones"
        );
        update_submodules(target, terminal_prompt, false).await?;
    }

    Ok(())
}

/// Snapshot tracked and staged local changes as a deterministic commit in a bare source.
pub(crate) async fn create_repo_snapshot(
    source: &Path,
    worktree: &Path,
    head_commit: &str,
    scratch: &Path,
) -> Result<String, Error> {
    let temp = tempfile::tempdir_in(scratch)?;
    let index = temp.path().join("index");
    let objects = source.join("objects");

    git_cmd()?
        .current_dir(worktree)
        .arg("read-tree")
        .arg(head_commit)
        .remove_git_envs()
        .env("GIT_INDEX_FILE", &index)
        .env("GIT_OBJECT_DIRECTORY", &objects)
        .check(true)
        .output()
        .await?;

    let staged_files = get_staged_files(worktree).await?;
    if !staged_files.is_empty() {
        git_cmd()?
            .current_dir(worktree)
            .arg("add")
            .arg("--")
            .file_args(&staged_files)
            .remove_git_envs()
            .env("GIT_INDEX_FILE", &index)
            .env("GIT_OBJECT_DIRECTORY", &objects)
            .check(true)
            .output()
            .await?;
    }

    git_cmd()?
        .current_dir(worktree)
        .arg("add")
        .arg("--update")
        .remove_git_envs()
        .env("GIT_INDEX_FILE", &index)
        .env("GIT_OBJECT_DIRECTORY", &objects)
        .check(true)
        .output()
        .await?;

    let tree = git_cmd()?
        .current_dir(worktree)
        .arg("write-tree")
        .remove_git_envs()
        .env("GIT_INDEX_FILE", &index)
        .env("GIT_OBJECT_DIRECTORY", &objects)
        .check(true)
        .output()
        .await?;
    let tree = String::from_utf8_lossy(&tree.stdout)
        .trim_ascii()
        .to_string();

    let commit = git_cmd()?
        .current_dir(worktree)
        .arg("commit-tree")
        .arg(&tree)
        .arg("-p")
        .arg(head_commit)
        .arg("-m")
        .arg("Temporary commit by prek try-repo")
        .remove_git_envs()
        .env("GIT_OBJECT_DIRECTORY", &objects)
        .env("GIT_AUTHOR_NAME", "prek try-repo")
        .env("GIT_AUTHOR_EMAIL", "try-repo@prek.dev")
        .env("GIT_COMMITTER_NAME", "prek try-repo")
        .env("GIT_COMMITTER_EMAIL", "try-repo@prek.dev")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .check(true)
        .output()
        .await?;
    let commit = String::from_utf8_lossy(&commit.stdout)
        .trim_ascii()
        .to_string();
    pin_repo_source_commit(source, &commit).await?;

    Ok(commit)
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
    cmd.remove_git_envs()
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
        .remove_git_envs()
        .env(EnvVars::LC_ALL, "C")
        .check(true)
        .output()
        .await?;

    Ok(output
        .stdout
        .split(|&byte| byte == b'\0')
        .any(|entry| entry.starts_with(b"160000 ")))
}

async fn get_config_value(scope: Option<&str>, key: &str) -> Result<Option<Vec<u8>>, Error> {
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
    Ok(get_config_value(scope, key).await?.is_some())
}

async fn config_value_is_empty(scope: Option<&str>, key: &str) -> Result<bool, Error> {
    Ok(get_config_value(scope, key)
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
fn shared_repository_file_mode(value: &str, mode: u32) -> Option<u32> {
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
pub(crate) async fn get_shared_repository_file_mode(mode: u32) -> Result<u32> {
    let output = git_cmd()?
        .arg("config")
        .arg("--get")
        .arg("core.sharedRepository")
        .check(false)
        .output()
        .await?;
    if output.status.success() {
        let value = str::from_utf8(&output.stdout)?;
        Ok(shared_repository_file_mode(value, mode).unwrap_or(mode))
    } else {
        Ok(mode)
    }
}

pub(crate) async fn get_lfs_files(
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
            stdin.write_all(path.to_string_lossy().as_bytes()).await?;
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
    let read_result = String::from_utf8_lossy(&read_result);
    let mut it = read_result.split_terminator('\0');
    while let (Some(file), Some(_attr), Some(value)) = (it.next(), it.next(), it.next()) {
        if value == "lfs" {
            lfs_files.insert(PathBuf::from(file));
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

/// Get commits that are ancestors of the given commit but not in the specified remote
pub(crate) async fn get_ancestors_not_in_remote(
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

/// Get root commits (commits with no parents) for the given commit
pub(crate) async fn get_root_commits(local_sha: &str) -> Result<FxHashSet<String>, Error> {
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

/// Get the parent commit of the given commit
pub(crate) async fn get_parent_commit(commit: &str) -> Result<Option<String>, Error> {
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
    let output = std::process::Command::new(git)
        .current_dir(git_root)
        .arg("config")
        .arg("--file")
        .arg(".gitmodules")
        .arg("--get-regexp")
        .arg(r"^submodule\..*\.path$")
        .output()?;

    Ok(String::from_utf8_lossy(&output.stdout)
        .trim_ascii()
        .lines()
        .filter_map(|line| line.split_whitespace().nth(1))
        .map(|submodule| git_root.join(submodule))
        .collect())
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::zsplit;
    use super::{
        Error, GIT, TerminalPrompt, checkout_repo_from_source, ensure_bare_repo,
        fetch_repo_source_for_update, fetch_repo_source_revision, shared_repository_file_mode,
        should_update_submodules, update_submodules,
    };
    use assert_cmd::assert::OutputAssertExt;
    use std::path::Path;
    use std::process::Command;

    fn run_git(path: &Path, args: &[&str]) {
        let mut command = Command::new(GIT.as_ref().unwrap());
        command.current_dir(path).args(args);

        command.assert().success();
    }

    fn git_stdout(path: &Path, args: &[&str]) -> String {
        let output = Command::new(GIT.as_ref().unwrap())
            .current_dir(path)
            .args(args)
            .output()
            .unwrap();
        assert!(output.status.success());
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    }

    fn commit_all(path: &Path, message: &str) {
        run_git(path, &["add", "--all"]);
        run_git(
            path,
            &[
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=prek",
                "-c",
                "user.email=prek@example.com",
                "commit",
                "-m",
                message,
            ],
        );
    }

    #[tokio::test]
    async fn should_update_submodules_when_gitmodules_exists() {
        let tmp = tempfile::tempdir().unwrap();
        run_git(tmp.path(), &["init"]);
        fs_err::write(tmp.path().join(".gitmodules"), "").unwrap();

        assert!(should_update_submodules(tmp.path()).await.unwrap());
    }

    #[tokio::test]
    async fn repo_source_update_refreshes_moved_and_deleted_tags() {
        let remote = tempfile::tempdir().unwrap();
        run_git(remote.path(), &["init"]);
        run_git(
            remote.path(),
            &[
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=prek",
                "-c",
                "user.email=prek@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "first",
            ],
        );
        let first = Command::new(GIT.as_ref().unwrap())
            .current_dir(remote.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let first = String::from_utf8_lossy(&first.stdout).trim().to_string();
        run_git(remote.path(), &["-c", "tag.gpgsign=false", "tag", "v1"]);

        let source_root = tempfile::tempdir().unwrap();
        let source = source_root.path().join("source.git");
        ensure_bare_repo(remote.path().to_str().unwrap(), &source)
            .await
            .unwrap();
        fetch_repo_source_for_update(&source).await.unwrap();
        let fetched_first = Command::new(GIT.as_ref().unwrap())
            .current_dir(&source)
            .args(["rev-parse", "refs/tags/v1"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&fetched_first.stdout).trim(), first);

        run_git(
            remote.path(),
            &[
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=prek",
                "-c",
                "user.email=prek@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "second",
            ],
        );
        let second = Command::new(GIT.as_ref().unwrap())
            .current_dir(remote.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let second = String::from_utf8_lossy(&second.stdout).trim().to_string();
        run_git(
            remote.path(),
            &["-c", "tag.gpgsign=false", "tag", "-f", "v1"],
        );
        run_git(remote.path(), &["-c", "tag.gpgsign=false", "tag", "v2"]);

        fetch_repo_source_for_update(&source).await.unwrap();
        let moved = Command::new(GIT.as_ref().unwrap())
            .current_dir(&source)
            .args(["rev-parse", "refs/tags/v1"])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&moved.stdout).trim(), second);

        run_git(remote.path(), &["tag", "--delete", "v2"]);
        fetch_repo_source_for_update(&source).await.unwrap();
        let deleted = Command::new(GIT.as_ref().unwrap())
            .current_dir(&source)
            .args(["show-ref", "--verify", "--quiet", "refs/tags/v2"])
            .status()
            .unwrap();
        assert!(!deleted.success());
    }

    #[tokio::test]
    async fn repo_source_full_fetches_unshallow_existing_sources() {
        let remote = tempfile::tempdir().unwrap();
        run_git(remote.path(), &["init"]);
        run_git(
            remote.path(),
            &[
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=prek",
                "-c",
                "user.email=prek@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "first",
            ],
        );
        let first = Command::new(GIT.as_ref().unwrap())
            .current_dir(remote.path())
            .args(["rev-parse", "HEAD"])
            .output()
            .unwrap();
        let first = String::from_utf8_lossy(&first.stdout).trim().to_string();
        let first_short = &first[..7];
        run_git(
            remote.path(),
            &[
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=prek",
                "-c",
                "user.email=prek@example.com",
                "commit",
                "--allow-empty",
                "-m",
                "second",
            ],
        );

        let source_root = tempfile::tempdir().unwrap();
        let update_source = source_root.path().join("update.git");
        ensure_bare_repo(remote.path().to_str().unwrap(), &update_source)
            .await
            .unwrap();
        fetch_repo_source_revision(&update_source, "HEAD", TerminalPrompt::Disabled)
            .await
            .unwrap();
        assert!(update_source.join("shallow").is_file());

        fetch_repo_source_for_update(&update_source).await.unwrap();
        assert!(!update_source.join("shallow").exists());
        Command::new(GIT.as_ref().unwrap())
            .args([
                "--git-dir",
                update_source.to_str().unwrap(),
                "cat-file",
                "-e",
            ])
            .arg(format!("{first}^{{commit}}"))
            .assert()
            .success();

        let fallback_source = source_root.path().join("fallback.git");
        ensure_bare_repo(remote.path().to_str().unwrap(), &fallback_source)
            .await
            .unwrap();
        fetch_repo_source_revision(&fallback_source, "HEAD", TerminalPrompt::Disabled)
            .await
            .unwrap();
        assert!(fallback_source.join("shallow").is_file());

        let resolved =
            fetch_repo_source_revision(&fallback_source, first_short, TerminalPrompt::Disabled)
                .await
                .unwrap();
        assert_eq!(resolved, first);
        assert!(!fallback_source.join("shallow").exists());

        let cached =
            fetch_repo_source_revision(&fallback_source, &resolved, TerminalPrompt::Disabled)
                .await
                .unwrap();
        assert_eq!(cached, first);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn repo_source_materializes_only_the_checkout_snapshot_after_update() {
        let remote = tempfile::tempdir().unwrap();
        run_git(remote.path(), &["init"]);
        fs_err::write(remote.path().join("old.py"), "print('old')\n").unwrap();
        commit_all(remote.path(), "old hook");
        let old_commit = git_stdout(remote.path(), &["rev-parse", "HEAD"]);
        let old_blob = git_stdout(remote.path(), &["rev-parse", "HEAD:old.py"]);

        fs_err::remove_file(remote.path().join("old.py")).unwrap();
        fs_err::write(remote.path().join("hook.py"), "print('hello')\n").unwrap();
        commit_all(remote.path(), "hook");
        // Local upload-pack disables filtering by default, unlike the hosted remotes this models.
        run_git(remote.path(), &["config", "uploadpack.allowFilter", "true"]);
        let repo = format!("file://{}", remote.path().display());

        let root = tempfile::tempdir().unwrap();
        let source = root.path().join("source.git");
        ensure_bare_repo(&repo, &source).await.unwrap();
        fetch_repo_source_for_update(&source).await.unwrap();
        let commit = git_stdout(&source, &["rev-parse", "FETCH_HEAD"]);

        let resolved = fetch_repo_source_revision(&source, &commit, TerminalPrompt::Disabled)
            .await
            .unwrap();
        assert_eq!(resolved, commit);
        assert!(
            !source.join("shallow").exists(),
            "materializing a known commit must preserve update's complete graph"
        );
        let old_snapshot = Command::new(GIT.as_ref().unwrap())
            .args([
                "--git-dir",
                source.to_str().unwrap(),
                "rev-list",
                "--objects",
                "--missing=print",
                "--no-object-names",
            ])
            .arg(format!("{old_commit}^{{tree}}"))
            .output()
            .unwrap();
        let missing_old_blob = format!("?{old_blob}");
        assert!(
            String::from_utf8_lossy(&old_snapshot.stdout)
                .lines()
                .any(|line| line == missing_old_blob),
            "materializing HEAD must not fetch blobs used only by its history"
        );

        let checkout = root.path().join("checkout");
        checkout_repo_from_source(&source, &repo, &commit, &checkout, TerminalPrompt::Disabled)
            .await
            .unwrap();
        assert_eq!(
            fs_err::read_to_string(checkout.join("hook.py")).unwrap(),
            "print('hello')\n"
        );
        assert_eq!(git_stdout(&checkout, &["rev-list", "--count", "HEAD"]), "1");
        Command::new(GIT.as_ref().unwrap())
            .current_dir(&checkout)
            .args(["fsck", "--full"])
            .assert()
            .success();
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
    fn shared_repository_group_mode_matches_git_behavior() {
        for value in ["group", "true", "yes", "on", "1"] {
            assert_eq!(shared_repository_file_mode(value, 0o755), Some(0o775));
        }
    }

    #[test]
    fn shared_repository_everybody_mode_matches_git_behavior() {
        for value in ["all", "world", "everybody", "2"] {
            assert_eq!(shared_repository_file_mode(value, 0o755), Some(0o775));
        }
    }

    #[test]
    fn shared_repository_octal_mode_matches_git_behavior() {
        assert_eq!(shared_repository_file_mode("0640", 0o644), Some(0o640));
        assert_eq!(shared_repository_file_mode("0640", 0o755), Some(0o750));
    }

    #[test]
    fn shared_repository_umask_or_invalid_values_do_not_override_mode() {
        for value in ["", "umask", "false", "no", "off", "0", "invalid", "0400"] {
            assert_eq!(shared_repository_file_mode(value, 0o755), None);
        }
    }
}
