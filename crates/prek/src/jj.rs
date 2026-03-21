use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use tracing::instrument;

use crate::process;
use crate::process::Cmd;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Command(#[from] process::Error),

    #[error("Failed to find Jujutsu (jj): {0}")]
    JjNotFound(#[from] which::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Path to the `jj` executable, resolved via `PATH`.
pub(crate) static JJ: LazyLock<Result<PathBuf, which::Error>> =
    LazyLock::new(|| which::which("jj"));

/// Walk up from `start` looking for a directory containing `.jj/`.
pub(crate) fn find_workspace_root(start: &Path) -> Option<PathBuf> {
    let mut current = start.to_path_buf();
    loop {
        if current.join(".jj").is_dir() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

fn resolve_repo_dir(workspace_root: &Path) -> Result<Option<PathBuf>, Error> {
    let repo_dir_candidate = workspace_root.join(".jj").join("repo");
    if repo_dir_candidate.is_file() {
        let content = fs_err::read_to_string(&repo_dir_candidate)?;
        let path = PathBuf::from(content.trim());
        let repo_dir = if path.is_absolute() {
            path
        } else {
            workspace_root.join(".jj").join(path)
        };
        return Ok(Some(repo_dir));
    }
    if repo_dir_candidate.is_dir() {
        return Ok(Some(repo_dir_candidate));
    }
    Ok(None)
}

/// Resolve the backing Git directory for a Jujutsu workspace.
///
/// For a primary (colocated) workspace, `.jj/repo` is a directory.
/// For a secondary workspace (created with `jj workspace add`), `.jj/repo` is a file
/// containing the absolute path to the main repo's `.jj/repo` directory.
pub(crate) fn resolve_backing_git_dir(workspace_root: &Path) -> Result<Option<PathBuf>, Error> {
    let Some(repo_dir) = resolve_repo_dir(workspace_root)? else {
        return Ok(None);
    };

    let git_target_file = repo_dir.join("store").join("git_target");
    let git_target = fs_err::read_to_string(&git_target_file)?;
    let git_target = git_target.trim();

    let git_path = PathBuf::from(git_target);
    let git_dir = if git_path.is_absolute() {
        git_path
    } else {
        repo_dir.join("store").join(git_path)
    };
    let git_dir = git_dir.canonicalize()?;

    if git_dir.exists() {
        Ok(Some(git_dir))
    } else {
        Ok(None)
    }
}

/// Create a new `Cmd` for running Jujutsu.
pub(crate) fn jj_cmd(summary: &str) -> Result<Cmd, Error> {
    let cmd = Cmd::new(JJ.as_ref().map_err(|&e| Error::JjNotFound(e))?, summary);
    Ok(cmd)
}

fn relative_to_cwd<'a>(cwd: &'a Path, path: &'a Path) -> &'a Path {
    path.strip_prefix(cwd)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(path)
}

fn parse_path_lines(output: &[u8]) -> Vec<PathBuf> {
    String::from_utf8_lossy(output)
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect()
}

/// List tracked files in a Jujutsu workspace revision.
#[instrument(level = "trace")]
pub(crate) async fn ls_files(cwd: &Path, path: &Path) -> Result<Vec<PathBuf>, Error> {
    let relative = relative_to_cwd(cwd, path);
    let mut cmd = jj_cmd("jj file list")?;
    cmd.current_dir(cwd).arg("file").arg("list");
    if !relative.as_os_str().is_empty() && relative != Path::new(".") {
        cmd.arg(relative);
    }

    let output = cmd.check(true).output().await?;
    Ok(parse_path_lines(&output.stdout))
}

/// Get the list of changed files in the current Jujutsu working copy.
#[instrument(level = "trace")]
pub(crate) async fn get_changed_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = jj_cmd("jj diff")?
        .current_dir(root)
        .arg("diff")
        .arg("-r")
        .arg("@")
        .arg("--name-only")
        .check(true)
        .output()
        .await?;
    Ok(parse_path_lines(&output.stdout))
}

/// Get the list of changed files between two Jujutsu revisions.
#[instrument(level = "trace")]
pub(crate) async fn get_changed_files_between(
    old: &str,
    new: &str,
    root: &Path,
) -> Result<Vec<PathBuf>, Error> {
    let output = jj_cmd("jj diff")?
        .current_dir(root)
        .arg("diff")
        .arg("--from")
        .arg(old)
        .arg("--to")
        .arg(new)
        .arg("--name-only")
        .check(true)
        .output()
        .await?;
    Ok(parse_path_lines(&output.stdout))
}

/// Get conflicted files in the current Jujutsu working copy.
#[instrument(level = "trace")]
pub(crate) async fn get_conflicted_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let output = jj_cmd("jj diff")?
        .current_dir(root)
        .arg("diff")
        .arg("-r")
        .arg("@")
        .arg("--types")
        .check(true)
        .output()
        .await?;

    let files = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() {
                return None;
            }

            let mut parts = line.splitn(2, char::is_whitespace);
            let status = parts.next()?;
            let path = parts.next()?.trim_start();
            if status.contains('C') && !path.is_empty() {
                Some(PathBuf::from(path))
            } else {
                None
            }
        })
        .collect();

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn find_workspace_root_returns_none_for_non_jj_directory() {
        let dir = tempfile::tempdir().unwrap();
        assert!(find_workspace_root(dir.path()).is_none());
    }

    #[test]
    fn find_workspace_root_finds_current_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::create_dir(dir.path().join(".jj")).unwrap();
        let result = find_workspace_root(dir.path());
        assert_eq!(result, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn find_workspace_root_finds_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::create_dir(dir.path().join(".jj")).unwrap();
        let child = dir.path().join("subdir");
        fs_err::create_dir(&child).unwrap();
        let result = find_workspace_root(&child);
        assert_eq!(result, Some(dir.path().to_path_buf()));
    }

    #[test]
    fn resolve_backing_git_dir_returns_none_without_repo_metadata() {
        let dir = tempfile::tempdir().unwrap();
        fs_err::create_dir(dir.path().join(".jj")).unwrap();
        let result = resolve_backing_git_dir(dir.path()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn resolve_backing_git_dir_resolves_colocated_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();

        let store_dir = root.join(".jj").join("repo").join("store");
        fs_err::create_dir_all(&store_dir).unwrap();
        fs_err::write(store_dir.join("git_target"), "../../../.git").unwrap();
        let git_dir = root.join(".git");
        fs_err::create_dir(&git_dir).unwrap();

        let resolved = resolve_backing_git_dir(root).unwrap();
        assert_eq!(resolved, Some(git_dir.canonicalize().unwrap()));
    }

    #[test]
    fn resolve_backing_git_dir_resolves_secondary_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let main_root = dir.path().join("main");
        let secondary_root = dir.path().join("secondary");

        let main_store = main_root.join(".jj").join("repo").join("store");
        fs_err::create_dir_all(&main_store).unwrap();
        fs_err::write(main_store.join("git_target"), "../../../.git").unwrap();
        let main_git = main_root.join(".git");
        fs_err::create_dir(&main_git).unwrap();

        let secondary_jj = secondary_root.join(".jj");
        fs_err::create_dir_all(&secondary_jj).unwrap();
        let main_repo_abs = main_root.join(".jj").join("repo").canonicalize().unwrap();
        fs_err::write(
            secondary_jj.join("repo"),
            main_repo_abs.to_string_lossy().as_ref(),
        )
        .unwrap();

        let resolved = resolve_backing_git_dir(&secondary_root).unwrap();
        assert_eq!(resolved, Some(main_git.canonicalize().unwrap()));
    }
}
