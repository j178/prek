use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
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
            let root = canonicalize_root(root);
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
            .map_err(|err| anyhow::anyhow!("Not inside a Git or Jujutsu repository: {err}"))?;
        let root = canonicalize_root(root);
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

/// Canonicalize the workspace root so `GIT_ROOT` and `repo::root()` agree on a single
/// path spelling (this mirrors the historic `GIT_ROOT` behavior). Falls back to the
/// original path if canonicalization fails.
fn canonicalize_root(root: PathBuf) -> PathBuf {
    dunce::canonicalize(&root).unwrap_or(root)
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
        Err(err) => Err(anyhow::anyhow!("{err:#}")),
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
    // Both backends report paths relative to `workspace_root` here (git via
    // `--relative`, jj by running in that directory), matching the project-relative
    // filenames hooks expect.
    match current()?.kind() {
        RepoKind::Git => git::get_added_files(workspace_root)
            .await
            .map_err(Into::into),
        RepoKind::Jujutsu => jj::get_added_files(workspace_root)
            .await
            .map_err(Into::into),
    }
}

/// Return the default file set for `prek run` when the user did not specify one.
///
/// The goal is to preserve each VCS's natural workflow:
/// Git uses staged files, while Jujutsu uses the current working-copy changeset.
pub(crate) async fn default_files(workspace_root: &Path) -> Result<Vec<PathBuf>> {
    let repo = current()?;
    match repo.kind() {
        RepoKind::Git => git::get_staged_files(workspace_root)
            .await
            .map_err(Into::into),
        // Run jj from the workspace root so paths are root-relative, matching git's
        // output; `collect_run_input` then strips the project prefix.
        RepoKind::Jujutsu => jj::get_changed_files(repo.root()).await.map_err(Into::into),
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
    let repo = current()?;
    match repo.kind() {
        RepoKind::Git => git::get_changed_files(old, new, workspace_root)
            .await
            .map_err(Into::into),
        RepoKind::Jujutsu => jj::get_changed_files_between(old, new, repo.root())
            .await
            .map_err(Into::into),
    }
}

/// List tracked files under the given pathspecs using the active repository backend.
///
/// This keeps callers focused on "which files belong to the repo here?" rather
/// than the mechanics of `git ls-files` versus the Jujutsu equivalent.
pub(crate) async fn ls_files<P>(
    cwd: &Path,
    paths: impl IntoIterator<Item = P>,
) -> Result<Vec<PathBuf>>
where
    P: AsRef<Path>,
{
    match current()?.kind() {
        RepoKind::Git => git::ls_files(cwd, paths).await.map_err(Into::into),
        RepoKind::Jujutsu => jj::ls_files(cwd, paths).await.map_err(Into::into),
    }
}

/// Return conflicted files if the current repo backend reports a conflict state.
///
/// Git exposes a repo-wide merge-conflict mode, while Jujutsu exposes conflicted
/// paths in the working copy. This helper normalizes both into "Some(files)" or
/// `None` so higher-level run logic can stay backend-agnostic.
pub(crate) async fn conflicted_files(workspace_root: &Path) -> Result<Option<Vec<PathBuf>>> {
    let repo = current()?;
    match repo.kind() {
        RepoKind::Git => {
            if git::is_in_merge_conflict().await? {
                Ok(Some(git::get_conflicted_files(workspace_root).await?))
            } else {
                Ok(None)
            }
        }
        RepoKind::Jujutsu => {
            let files = jj::get_conflicted_files(repo.root()).await?;
            if files.is_empty() {
                Ok(None)
            } else {
                Ok(Some(files))
            }
        }
    }
}
