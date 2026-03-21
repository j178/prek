use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use rustc_hash::FxHashSet;
use tracing::debug;

use crate::git;
use crate::jj;
use crate::process::Cmd;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RepoKind {
    Git,
    Jujutsu,
}

#[derive(Debug)]
pub(crate) struct RepoContext {
    kind: RepoKind,
    root: PathBuf,
    backing_git_dir: Option<PathBuf>,
}

impl RepoContext {
    /// Detect the repository model for the current working directory once at startup.
    ///
    /// The intent here is to give the rest of the codebase a single answer to
    /// "what kind of repo am I in?" after `--cd` and other cwd changes have already
    /// taken effect. Jujutsu is checked first because a Jujutsu workspace may not be
    /// discoverable via plain Git root detection from the current directory.
    fn detect_current() -> Result<Self> {
        let cwd = std::env::current_dir().context("Failed to get current directory")?;
        if let Some(root) = jj::find_workspace_root(&cwd) {
            let git_dir = jj::resolve_backing_git_dir(&root)
                .context("Failed to resolve backing Git directory for Jujutsu workspace")?
                .context(
                    "Detected a Jujutsu workspace, but could not resolve its backing Git directory",
                )?;
            debug!(
                root = %root.display(),
                git_dir = %git_dir.display(),
                "Detected Jujutsu workspace",
            );
            return Ok(Self {
                kind: RepoKind::Jujutsu,
                root,
                backing_git_dir: Some(git_dir),
            });
        }

        let root = git::get_root()
            .map_err(anyhow::Error::new)
            .map_err(|err| anyhow::anyhow!("Not inside a Git or Jujutsu repository: {err}"))?;
        debug!(root = %root.display(), "Detected Git repository");

        Ok(Self {
            kind: RepoKind::Git,
            root,
            backing_git_dir: None,
        })
    }

    pub(crate) fn kind(&self) -> RepoKind {
        self.kind
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    /// Apply per-command Git environment needed to talk to the backing Git repo.
    ///
    /// This is intentionally scoped to the specific command being built. We do not
    /// mutate process-wide `GIT_DIR` / `GIT_WORK_TREE`, because that would make
    /// Jujutsu support an ambient global side effect rather than an explicit repo
    /// backend behavior.
    fn apply_git_env(&self, cmd: &mut Cmd) {
        if let Some(git_dir) = &self.backing_git_dir {
            cmd.env(EnvVars::GIT_DIR, git_dir);
            cmd.env(EnvVars::GIT_WORK_TREE, &self.root);
        }
    }
}

pub(crate) static REPO_CONTEXT: LazyLock<Result<RepoContext>> =
    LazyLock::new(RepoContext::detect_current);

/// Access the cached repository context with a normal `Result` API.
///
/// `LazyLock<Result<_>>` is convenient for one-time detection, but most callers
/// want error propagation instead of reasoning about the lazy container directly.
fn current() -> Result<&'static RepoContext> {
    match REPO_CONTEXT.as_ref() {
        Ok(repo) => Ok(repo),
        Err(err) => Err(anyhow::anyhow!("{err}")),
    }
}

/// Apply repository-specific Git environment to a single command if needed.
///
/// For plain Git repositories this is a no-op. For Jujutsu workspaces this points
/// Git commands at the backing Git store so the rest of prek can keep using a
/// small number of Git-backed primitives without leaking that setup everywhere.
pub(crate) fn apply_git_env(cmd: &mut Cmd) {
    if let Ok(repo) = REPO_CONTEXT.as_ref() {
        repo.apply_git_env(cmd);
    }
}

/// Return the repository root that prek should treat as the workspace boundary.
///
/// This is the Jujutsu workspace root for Jujutsu repos and the Git root for
/// plain Git repos. Callers should prefer this instead of reaching into Git/JJ
/// discovery directly.
pub(crate) fn root() -> Result<&'static Path> {
    Ok(current()?.root())
}

/// Return the hooks directory for the active repository backend.
///
/// Jujutsu still uses the backing Git hooks directory, but that detail stays
/// behind this function so install and hook entrypoints do not need to know it.
pub(crate) async fn hooks_dir() -> Result<PathBuf> {
    Ok(git::get_git_common_dir().await?.join("hooks"))
}

/// Whether `prek run` should preserve Git's stash/clean-worktree behavior by default.
///
/// Git's default mode is index-driven, so stashing protects unstaged changes from
/// bleeding into hook execution. Jujutsu's default mode is working-copy based, so
/// that Git-specific hygiene step does not apply.
pub(crate) fn should_stash_by_default_run() -> bool {
    current()
        .map(|repo| repo.kind() == RepoKind::Git)
        .unwrap_or(true)
}

/// Whether config files must be staged before they are considered authoritative.
///
/// This is a Git-specific rule because prek historically reads config from the
/// staged snapshot. Jujutsu has no staging area, so enforcing that rule there
/// would be both confusing and wrong.
pub(crate) fn requires_staged_configs() -> bool {
    current()
        .map(|repo| repo.kind() == RepoKind::Git)
        .unwrap_or(true)
}

/// Return files that should be treated as newly introduced for hook logic.
///
/// In Git this maps to added files in the index. In Jujutsu there is no staging
/// area, so the closest useful intent is "files changed in the current working
/// copy changeset".
pub(crate) async fn added_files(workspace_root: &Path) -> Result<Vec<PathBuf>> {
    match current()?.kind() {
        RepoKind::Git => git::get_added_files(workspace_root)
            .await
            .map_err(Into::into),
        RepoKind::Jujutsu => jj::get_changed_files(workspace_root)
            .await
            .map_err(Into::into),
    }
}

/// Return the default file set for `prek run` when the user did not specify one.
///
/// The goal is to preserve each VCS's natural workflow:
/// Git uses staged files, while Jujutsu uses the current working-copy changeset.
pub(crate) async fn default_files(workspace_root: &Path) -> Result<Vec<PathBuf>> {
    match current()?.kind() {
        RepoKind::Git => git::get_staged_files(workspace_root)
            .await
            .map_err(Into::into),
        RepoKind::Jujutsu => jj::get_changed_files(workspace_root)
            .await
            .map_err(Into::into),
    }
}

/// Return files changed between two user-supplied revisions.
///
/// The caller does not need to care whether those revision strings are Git refs or
/// Jujutsu revsets/bookmarks; each backend interprets them using its own native
/// revision syntax.
pub(crate) async fn changed_files_between(
    old: &str,
    new: &str,
    workspace_root: &Path,
) -> Result<Vec<PathBuf>> {
    match current()?.kind() {
        RepoKind::Git => git::get_changed_files(old, new, workspace_root)
            .await
            .map_err(Into::into),
        RepoKind::Jujutsu => jj::get_changed_files_between(old, new, workspace_root)
            .await
            .map_err(Into::into),
    }
}

/// List tracked files under `path` using the active repository backend.
///
/// This keeps callers focused on "which files belong to the repo here?" rather
/// than the mechanics of `git ls-files` versus the Jujutsu equivalent.
pub(crate) async fn ls_files(cwd: &Path, path: &Path) -> Result<Vec<PathBuf>> {
    match current()?.kind() {
        RepoKind::Git => git::ls_files(cwd, path).await.map_err(Into::into),
        RepoKind::Jujutsu => jj::ls_files(cwd, path).await.map_err(Into::into),
    }
}

/// Return conflicted files if the current repo backend reports a conflict state.
///
/// Git exposes a repo-wide merge-conflict mode, while Jujutsu exposes conflicted
/// paths in the working copy. This helper normalizes both into "Some(files)" or
/// `None` so higher-level run logic can stay backend-agnostic.
pub(crate) async fn conflicted_files(workspace_root: &Path) -> Result<Option<Vec<PathBuf>>> {
    match current()?.kind() {
        RepoKind::Git => {
            if git::is_in_merge_conflict().await? {
                Ok(Some(git::get_conflicted_files(workspace_root).await?))
            } else {
                Ok(None)
            }
        }
        RepoKind::Jujutsu => {
            let files = jj::get_conflicted_files(workspace_root).await?;
            if files.is_empty() {
                Ok(None)
            } else {
                Ok(Some(files))
            }
        }
    }
}

/// Report whether the backing Git repository stores executable-bit metadata.
///
/// The executable shebang hook only makes sense when the repository tracks mode
/// bits. Even in a Jujutsu workspace, that metadata still comes from the backing
/// Git store.
pub(crate) async fn tracks_executable_bit() -> Result<bool> {
    let stdout = git::git_cmd("get file file mode")?
        .arg("config")
        .arg("core.fileMode")
        .check(true)
        .output()
        .await?
        .stdout;
    Ok(std::str::from_utf8(&stdout)?.trim() != "false")
}

/// Return the subset of `filenames` that are marked executable in repo metadata.
///
/// This is intentionally repo metadata, not a filesystem stat call. We care about
/// which files the VCS records as executable because that is what hooks validate
/// and what collaborators will observe after commit/checkout.
pub(crate) async fn executable_files(
    file_base: &Path,
    filenames: &[&Path],
) -> Result<Vec<PathBuf>> {
    let filenames: FxHashSet<_> = filenames.iter().copied().collect();

    let output = git::git_cmd("git ls-files")?
        .arg("ls-files")
        .arg("--stage")
        .arg("-z")
        .arg("--")
        .arg(if file_base.as_os_str().is_empty() {
            Path::new(".")
        } else {
            file_base
        })
        .check(true)
        .output()
        .await?;

    let mut executable_files = Vec::new();
    for entry in output.stdout.split(|&b| b == b'\0') {
        let entry = std::str::from_utf8(entry)?;
        if entry.is_empty() {
            continue;
        }

        let mut parts = entry.split('\t');
        let Some(metadata) = parts.next() else {
            continue;
        };
        let file_name = match parts.next() {
            Some(file_name) => Path::new(file_name),
            None => continue,
        };
        if !filenames.contains(file_name) {
            continue;
        }

        let Some(mode_str) = metadata.split_whitespace().next() else {
            continue;
        };
        let Ok(mode_bits) = u32::from_str_radix(mode_str, 8) else {
            continue;
        };
        if (mode_bits & 0o111) == 0 {
            continue;
        }

        executable_files.push(
            file_name
                .strip_prefix(file_base)
                .unwrap_or(file_name)
                .to_path_buf(),
        );
    }

    Ok(executable_files)
}
