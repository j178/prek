//! Discover work trees using libgit2's configuration and cwd semantics.
//!
//! This includes libgit2's differences from Git for core.worktree in global or
//! included config, linked worktree overrides, and discovery from inside .git.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

pub(super) fn root(preserved_work_tree: Option<&Path>) -> Result<PathBuf> {
    // These overrides have different or incomplete semantics in libgit2.
    for name in [
        "GIT_WORK_TREE",
        "GIT_COMMON_DIR",
        "GIT_OBJECT_DIRECTORY",
        "GIT_IMPLICIT_WORK_TREE",
        "GIT_DISCOVERY_ACROSS_FILESYSTEM",
        "GIT_CEILING_DIRECTORIES",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_TEST_ASSUME_DIFFERENT_OWNER",
    ] {
        ensure!(!EnvVars.is_set(name), "{name} requires Git discovery");
    }
    if let Some(git_dir) = EnvVars.var_os(EnvVars::GIT_DIR) {
        ensure!(!git_dir.is_empty(), "GIT_DIR is empty");
    }

    let repo = git2::Repository::open_from_env()?;
    let root = repo.workdir().context("The repository is bare")?;
    if let Some(work_tree) = preserved_work_tree {
        // prek preserves the initial cwd when Git invokes a hook with GIT_DIR.
        ensure!(
            same_file::is_same_file(root, work_tree)?,
            "GIT_DIR requires the preserved work tree"
        );
    }
    Ok(root.to_path_buf())
}
