use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tracing::debug;

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

pub(crate) static JJ: LazyLock<Result<PathBuf, which::Error>> =
    LazyLock::new(|| which::which("jj"));

/// Detect if we're inside a jj workspace by walking up from CWD looking for a `.jj/` directory.
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
    if std::env::var_os("GIT_DIR").is_some() {
        return;
    }

    let Some(git_dir) = detect_jj_git_dir() else {
        return;
    };

    let cwd = match std::env::current_dir() {
        Ok(cwd) => cwd,
        Err(_) => return,
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
        std::env::set_var("GIT_DIR", &git_dir);
        std::env::set_var("GIT_WORK_TREE", &workspace_root);
    }
}

pub(crate) fn jj_cmd(summary: &str) -> Result<Cmd, Error> {
    let cmd = Cmd::new(JJ.as_ref().map_err(|&e| Error::JjNotFound(e))?, summary);
    Ok(cmd)
}

/// Get the list of changed files in the current jj working copy.
///
/// Uses `jj diff --name-only` which lists files changed in the current changeset.
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
