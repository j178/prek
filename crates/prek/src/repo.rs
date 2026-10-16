//! Repository-backend dispatch.
//!
//! prek is Git-first: `git.rs` owns root detection and the Git plumbing, and Jujutsu
//! workspaces are driven through their backing Git store. This module holds the few
//! queries whose answer *differs* by backend, so callers ask for the behavior rather
//! than for a specific VCS.
//!
//! Detection itself lives in `git.rs` (`git::root`, `git::is_jujutsu`), because the
//! workspace boundary and the Git directories are discovered together.

use std::path::{Path, PathBuf};

use anyhow::Result;

use crate::git::{self, FileEntry};
use crate::jj;
use crate::process::Cmd;

/// Whether the repository prek is running in is a Jujutsu workspace.
pub(crate) fn is_jujutsu() -> bool {
    git::is_jujutsu()
}

/// The repository boundary: the Jujutsu workspace root or the Git root.
fn root() -> Result<&'static Path> {
    Ok(git::root()?)
}

/// Whether `prek run` should keep Git's stash/clean-worktree behavior by default.
///
/// Git's default mode is index-driven, so stashing protects unstaged changes from
/// bleeding into hook execution. Jujutsu's default mode is working-copy based, so
/// that Git-specific hygiene step does not apply.
pub(crate) fn should_stash_by_default_run() -> bool {
    !is_jujutsu()
}

/// Whether config files must be staged before they are considered authoritative.
///
/// This is a Git-specific rule because prek historically reads config from the
/// staged snapshot. Jujutsu has no staging area, so enforcing that rule there
/// would be both confusing and wrong.
pub(crate) fn requires_staged_configs() -> bool {
    !is_jujutsu()
}

/// Files to treat as newly introduced: added-in-index for Git, absent-in-parent
/// for Jujutsu (which has no staging area).
///
/// Both backends report paths relative to `workspace_root` here (git via
/// `--relative`, jj by rebasing its workspace-relative output), matching the
/// project-relative filenames hooks expect.
pub(crate) async fn added_files(workspace_root: &Path) -> Result<Vec<PathBuf>> {
    if is_jujutsu() {
        jj::get_added_files(root()?, workspace_root)
            .await
            .map_err(Into::into)
    } else {
        git::staged_added_files(workspace_root)
            .await
            .map_err(Into::into)
    }
}

/// Default file set for `prek run`: staged files for Git, the working-copy
/// changeset for Jujutsu.
///
/// Results are repository-root-relative, like the Git backend's.
pub(crate) async fn default_files(
    workspace_root: &Path,
    include_deleted: bool,
) -> Result<Vec<FileEntry>> {
    if is_jujutsu() {
        // Run jj from the workspace root so paths are repository-relative, matching
        // git's output; `collect_run_input` then strips the project prefix. The scope
        // keeps the diff under the project for nested workspaces.
        jj::get_changed_files(root()?, Some(workspace_root), include_deleted)
            .await
            .map_err(Into::into)
    } else {
        git::staged_files(workspace_root, include_deleted)
            .await
            .map_err(Into::into)
    }
}

/// Files changed in the commit that was just completed (`--last-commit`).
///
/// Git spells this commit pair `HEAD~1`/`HEAD`; in Jujutsu the working-copy commit is a new
/// child of the commit that was completed, so the pair is resolved through the backend.
pub(crate) async fn last_commit_files(
    workspace_root: &Path,
    include_deleted: bool,
) -> Result<Vec<FileEntry>> {
    if is_jujutsu() {
        jj::get_last_commit_files(root()?, Some(workspace_root), include_deleted)
            .await
            .map_err(Into::into)
    } else {
        git::changed_files("HEAD~1", "HEAD", workspace_root, include_deleted)
            .await
            .map_err(Into::into)
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
    include_deleted: bool,
) -> Result<Vec<FileEntry>> {
    if is_jujutsu() {
        jj::get_changed_files_between(old, new, root()?, Some(workspace_root), include_deleted)
            .await
            .map_err(Into::into)
    } else {
        git::changed_files(old, new, workspace_root, include_deleted)
            .await
            .map_err(Into::into)
    }
}

/// List tracked files under `paths`, relative to `cwd`, using the active backend.
pub(crate) async fn ls_files<P>(
    cwd: &Path,
    paths: impl IntoIterator<Item = P>,
) -> Result<Vec<PathBuf>>
where
    P: AsRef<Path>,
{
    if is_jujutsu() {
        jj::ls_files(root()?, cwd, paths).await.map_err(Into::into)
    } else {
        git::ls_files(cwd, paths).await.map_err(Into::into)
    }
}

/// Conflicted files, relative to the repository root, or `None` when there are none.
///
/// Git uses its repository-wide merge-conflict state; Jujutsu records conflicts in the
/// working-copy commit.
pub(crate) async fn conflicted_files(workspace_root: &Path) -> Result<Option<Vec<PathBuf>>> {
    if is_jujutsu() {
        let files = jj::get_conflicted_files(root()?)
            .await
            .map_err(anyhow::Error::from)?;
        Ok((!files.is_empty()).then_some(files))
    } else if git::is_in_merge_conflict()? {
        Ok(Some(git::conflicted_files(workspace_root).await?))
    } else {
        Ok(None)
    }
}

/// The Git root of the repository `path` belongs to, when that repository is plain Git rather
/// than a Jujutsu workspace.
///
/// The nearest boundary wins, as it does for prek's own discovery: a Git repository nested inside
/// a Jujutsu workspace is its own repository, and a Jujutsu workspace nested inside a Git checkout
/// is a workspace.
pub(crate) async fn plain_git_root(path: &Path) -> Result<Option<PathBuf>> {
    let Some(git_root) = git::root_at(path).await? else {
        return Ok(None);
    };
    let git_root = dunce::canonicalize(&git_root).unwrap_or(git_root);

    match jj::find_workspace_root(path) {
        Some(jujutsu_root) => {
            let jujutsu_root = dunce::canonicalize(&jujutsu_root).unwrap_or(jujutsu_root);
            if jujutsu_root.starts_with(&git_root) {
                return Ok(None);
            }
            Ok(Some(git_root))
        }
        None => Ok(Some(git_root)),
    }
}

/// Working-copy changes under `path`, used to detect hooks that rewrite files.
///
/// Samples are only compared with each other, never parsed, so each backend can use its
/// own format. Jujutsu workspaces must be sampled through jj, since a Git diff there
/// changes whenever jj snapshots, with no file changing on disk. The backend comes from `path`
/// rather than from where prek started, since a Git repository nested inside a Jujutsu workspace
/// is not part of the workspace's changeset.
pub(crate) async fn worktree_diff(path: &Path) -> Result<Vec<u8>> {
    if plain_git_root(path).await?.is_some() {
        git::diff_worktree(path).await.map_err(Into::into)
    } else {
        jj::diff_worktree(root()?, Some(path))
            .await
            .map_err(Into::into)
    }
}

/// Command that prints the working-copy patch for `--show-diff-on-failure`.
///
/// Modification detection and diff display have to agree on what changed, so the displayed
/// patch comes from the same backend that tracks the changes.
pub(crate) async fn worktree_diff_cmd(root: &Path, color: &str) -> Result<Cmd> {
    if plain_git_root(root).await?.is_some() {
        git::diff_worktree_cmd(root, color).map_err(Into::into)
    } else {
        jj::diff_worktree_cmd(root, color).map_err(Into::into)
    }
}

/// A revision a hook can pass to Git: `reference` as given outside Jujutsu, and the backing
/// store's commit ID inside it.
///
/// `None` for no reference at all, and for a revision Jujutsu resolves to its root commit,
/// whose all-zero ID Git rejects in a diff range.
pub(crate) async fn hook_commit_id(reference: Option<&str>) -> Result<Option<String>> {
    let Some(reference) = reference else {
        return Ok(None);
    };
    if !is_jujutsu() {
        return Ok(Some(reference.to_owned()));
    }
    jj::hook_commit_id(reference).await.map_err(Into::into)
}
