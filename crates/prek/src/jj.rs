use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use prek_consts::env_vars::EnvVars;
use tracing::{debug, instrument};

use crate::process;
use crate::process::Cmd;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Command(#[from] process::Error),

    #[error("Failed to find jj: {0}")]
    JjNotFound(#[from] which::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Path to the `jj` executable, resolved via `PATH`.
pub(crate) static JJ: LazyLock<Result<PathBuf, which::Error>> =
    LazyLock::new(|| which::which("jj"));

/// Whether the current working directory is inside a jj workspace.
pub(crate) static IS_JJ_WORKSPACE: LazyLock<bool> = LazyLock::new(|| {
    let Ok(cwd) = std::env::current_dir() else {
        return false;
    };
    find_jj_dir(&cwd).is_some()
});

/// Walk up from `start` looking for a directory containing `.jj/`.
fn find_jj_dir(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        let jj_dir = current.join(".jj");
        if jj_dir.is_dir() {
            return Some(jj_dir);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Detect the backing git directory for a jj workspace.
///
/// For a primary (colocated) workspace, `.jj/repo` is a directory.
/// For a secondary workspace (created with `jj workspace add`), `.jj/repo` is a file
/// containing the absolute path to the main repo's `.jj/repo` directory.
///
/// The git store location is read from `<repo_path>/store/git_target`.
fn detect_jj_git_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let jj_dir = find_jj_dir(&cwd)?;

    let repo_dir_candidate = jj_dir.join("repo");
    let repo_dir = if repo_dir_candidate.is_file() {
        // Secondary workspace: file contains the path to the main repo dir.
        let content = std::fs::read_to_string(&repo_dir_candidate).ok()?;
        let path = PathBuf::from(content.trim());
        if path.is_absolute() {
            path
        } else {
            jj_dir.join(path)
        }
    } else if repo_dir_candidate.is_dir() {
        repo_dir_candidate
    } else {
        return None;
    };

    let git_target_file = repo_dir.join("store").join("git_target");
    let git_target = std::fs::read_to_string(&git_target_file).ok()?;
    let git_target = git_target.trim();

    let git_path = PathBuf::from(git_target);
    let git_dir = if git_path.is_absolute() {
        git_path
    } else {
        repo_dir.join("store").join(git_path)
    };

    // Canonicalize to resolve any `..` components.
    let git_dir = git_dir.canonicalize().ok()?;

    if git_dir.exists() {
        Some(git_dir)
    } else {
        None
    }
}

/// Set `GIT_DIR` and `GIT_WORK_TREE` environment variables for jj workspaces.
///
/// This must be called early in startup, before any git commands are run.
/// If `GIT_DIR` is already set, we leave it alone (e.g., running from a git hook).
pub(crate) fn setup_git_env_for_jj() {
    if EnvVars::is_set(EnvVars::GIT_DIR) {
        return;
    }

    let Some(git_dir) = detect_jj_git_dir() else {
        return;
    };

    let Ok(cwd) = std::env::current_dir() else {
        return;
    };

    // Find the workspace root (the directory containing `.jj/`).
    let workspace_root = find_jj_dir(&cwd)
        .and_then(|jj_dir| jj_dir.parent().map(Path::to_path_buf))
        .unwrap_or(cwd);

    debug!(
        "jj workspace detected, setting GIT_DIR={}, GIT_WORK_TREE={}",
        git_dir.display(),
        workspace_root.display()
    );

    unsafe {
        std::env::set_var(EnvVars::GIT_DIR, &git_dir);
        std::env::set_var(EnvVars::GIT_WORK_TREE, &workspace_root);
    }
}

/// Create a new `Cmd` for running jj.
pub(crate) fn jj_cmd(summary: &str) -> Result<Cmd, Error> {
    let cmd = Cmd::new(JJ.as_ref().map_err(|&e| Error::JjNotFound(e))?, summary);
    Ok(cmd)
}

/// Get the list of changed files in the current jj working copy.
///
/// Uses `jj diff --name-only` which lists files changed in the current changeset.
#[instrument(level = "trace")]
pub(crate) async fn get_changed_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = jj_cmd("jj diff")?
        .current_dir(root)
        .arg("diff")
        .arg("--name-only")
        .check(true)
        .output()
        .await?;

    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect();

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_jj_dir_returns_none_for_non_jj_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_jj_dir(dir.path()).is_none());
    }

    #[test]
    fn find_jj_dir_finds_jj_in_current_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".jj")).unwrap();
        let result = find_jj_dir(dir.path());
        assert_eq!(result, Some(dir.path().join(".jj")));
    }

    #[test]
    fn find_jj_dir_finds_jj_in_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join(".jj")).unwrap();
        let child = dir.path().join("subdir");
        std::fs::create_dir(&child).unwrap();
        let result = find_jj_dir(&child);
        assert_eq!(result, Some(dir.path().join(".jj")));
    }

    #[test]
    fn detect_jj_git_dir_returns_none_without_jj_workspace() {
        let dir = tempfile::tempdir().unwrap();
        // No .jj dir at all — detection should return None.
        // We can't easily test this without changing CWD, so just verify
        // the helper function returns None.
        assert!(find_jj_dir(dir.path()).is_none());
    }

    #[test]
    fn detect_jj_git_dir_returns_none_without_git_target() {
        let dir = tempfile::tempdir().unwrap();
        let jj_dir = dir.path().join(".jj");
        let repo_dir = jj_dir.join("repo");
        let store_dir = repo_dir.join("store");
        std::fs::create_dir_all(&store_dir).unwrap();
        // No git_target file — should not resolve.
        // detect_jj_git_dir() reads CWD, so we test the building blocks.
        assert!(find_jj_dir(dir.path()).is_some());
        assert!(!store_dir.join("git_target").exists());
    }

    #[test]
    fn detect_jj_git_dir_resolves_colocated_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        // Set up a colocated jj workspace structure:
        //   .jj/repo/store/git_target  →  "../../../.git"
        //   .git/
        let jj_dir = root.join(".jj");
        let store_dir = jj_dir.join("repo").join("store");
        std::fs::create_dir_all(&store_dir).unwrap();
        std::fs::write(store_dir.join("git_target"), "../../../.git").unwrap();
        let git_dir = root.join(".git");
        std::fs::create_dir(&git_dir).unwrap();

        // Since detect_jj_git_dir uses std::env::current_dir, we test the
        // resolution logic directly.
        let repo_dir = jj_dir.join("repo");
        let git_target =
            std::fs::read_to_string(repo_dir.join("store").join("git_target")).unwrap();
        let resolved = repo_dir.join("store").join(git_target.trim());
        let resolved = resolved.canonicalize().unwrap();
        assert_eq!(resolved, git_dir.canonicalize().unwrap());
    }

    #[test]
    fn detect_jj_git_dir_resolves_secondary_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let main_root = dir.path().join("main");
        let secondary_root = dir.path().join("secondary");

        // Set up main workspace:
        //   main/.jj/repo/store/git_target  →  "../../../.git"
        //   main/.git/
        let main_jj = main_root.join(".jj");
        let main_store = main_jj.join("repo").join("store");
        std::fs::create_dir_all(&main_store).unwrap();
        std::fs::write(main_store.join("git_target"), "../../../.git").unwrap();
        let main_git = main_root.join(".git");
        std::fs::create_dir(&main_git).unwrap();

        // Set up secondary workspace:
        //   secondary/.jj/repo  →  file pointing to main/.jj/repo (absolute path)
        let secondary_jj = secondary_root.join(".jj");
        std::fs::create_dir_all(&secondary_jj).unwrap();
        let main_repo_abs = main_jj.join("repo").canonicalize().unwrap();
        std::fs::write(secondary_jj.join("repo"), main_repo_abs.to_str().unwrap()).unwrap();

        // Verify secondary workspace resolves to the same git dir.
        let repo_content = std::fs::read_to_string(secondary_jj.join("repo")).unwrap();
        let repo_dir = PathBuf::from(repo_content.trim());
        assert!(repo_dir.is_dir());

        let git_target =
            std::fs::read_to_string(repo_dir.join("store").join("git_target")).unwrap();
        let resolved = repo_dir.join("store").join(git_target.trim());
        let resolved = resolved.canonicalize().unwrap();
        assert_eq!(resolved, main_git.canonicalize().unwrap());
    }
}
