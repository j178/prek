//! In-process repository discovery, with Git handling incompatible configuration.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use git2::{ConfigLevel, ErrorCode, Repository};
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

    let repo = Repository::open_from_env()?;
    let config = repo.config()?;
    // Git determines the work tree from the repository's own config before
    // resolving includes or loading global config. libgit2 uses the merged
    // config, so let Git handle entries whose origin would change that result.
    // Worktree config also differs for linked worktrees in libgit2.
    for name in ["core.bare", "core.worktree", "extensions.worktreeconfig"] {
        match config.get_entry(name) {
            Ok(entry) => {
                ensure!(
                    name != "extensions.worktreeconfig"
                        && entry.level() == ConfigLevel::Local
                        && entry.include_depth() == 0
                        && entry.has_value(),
                    "{name} requires Git discovery"
                );
            }
            Err(err) if err.code() == ErrorCode::NotFound => {}
            Err(err) => return Err(err.into()),
        }
    }

    let root = repo.workdir().context("The repository is bare")?;
    let root = dunce::canonicalize(root)?;
    if let Some(work_tree) = preserved_work_tree {
        // prek preserves the initial cwd when Git invokes a hook with GIT_DIR.
        // libgit2 instead infers the work tree from the repository layout.
        ensure!(
            same_file::is_same_file(&root, work_tree)?,
            "GIT_DIR requires the preserved work tree"
        );
    } else {
        // libgit2 returns a workdir even from inside .git, where Git may reject
        // commands that require a work tree.
        let cwd = dunce::canonicalize(std::env::current_dir()?)?;
        ensure!(
            !cwd.starts_with(dunce::canonicalize(repo.path())?)
                && !cwd.starts_with(dunce::canonicalize(repo.commondir())?),
            "The current directory is inside a Git directory"
        );
    }
    Ok(root)
}
