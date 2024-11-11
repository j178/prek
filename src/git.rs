use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use assert_cmd::output::{OutputError, OutputOkExt};
use tokio::process::Command;
use tracing::warn;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Command(#[from] OutputError),
    #[error("Failed to find git: {0}")]
    GitNotFound(#[from] which::Error),
}

pub static GIT: LazyLock<Result<PathBuf, which::Error>> = LazyLock::new(|| which::which("git"));

static GIT_ENV: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
    let keep = &[
        "GIT_EXEC_PATH",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
        "GIT_SSL_CAINFO",
        "GIT_SSL_NO_VERIFY",
        "GIT_CONFIG_COUNT",
        "GIT_HTTP_PROXY_AUTHMETHOD",
        "GIT_ALLOW_PROTOCOL",
        "GIT_ASKPASS",
    ];

    std::env::vars()
        .filter(|(k, _)| {
            !k.starts_with("GIT_")
                || k.starts_with("GIT_CONFIG_KEY_")
                || k.starts_with("GIT_CONFIG_VALUE_")
                || keep.contains(&k.as_str())
        })
        .collect()
});

fn git_cmd() -> Result<Command, Error> {
    let mut cmd = Command::new(GIT.clone()?);
    cmd.arg("-c").arg("core.useBuiltinFSMonitor=false");
    cmd.envs(GIT_ENV.iter().cloned());

    Ok(cmd)
}

fn zsplit(s: &[u8]) -> Vec<String> {
    let s = String::from_utf8_lossy(s);
    let s = s.trim_end_matches('\0');
    if s.is_empty() {
        vec![]
    } else {
        s.split('\0')
            .map(std::string::ToString::to_string)
            .collect()
    }
}

// TODO: improve error display
pub async fn get_changed_files(old: &str, new: &str) -> Result<Vec<String>, Error> {
    let output = git_cmd()?
        .arg("diff")
        .arg("--name-only")
        .arg("--diff-filter=ACMRT")
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg("-z") // Use NUL as line terminator
        .arg(format!("{old}...{new}"))
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(zsplit(&output.stdout))
}

pub async fn get_all_files() -> Result<Vec<String>, Error> {
    let output = git_cmd()?
        .arg("ls-files")
        .arg("-z")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(zsplit(&output.stdout))
}

pub async fn get_git_dir() -> Result<PathBuf, Error> {
    let output = git_cmd()?
        .arg("rev-parse")
        .arg("--git-dir")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

pub async fn get_git_common_dir() -> Result<PathBuf, Error> {
    let output = git_cmd()?
        .arg("rev-parse")
        .arg("--git-common-dir")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    if output.stdout.trim_ascii().is_empty() {
        Ok(get_git_dir().await?)
    } else {
        Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        ))
    }
}

pub async fn get_staged_files() -> Result<Vec<String>, Error> {
    let output = git_cmd()?
        .arg("diff")
        .arg("--staged")
        .arg("--name-only")
        .arg("--diff-filter=ACMRTUXB") // Everything except for D
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg("-z") // Use NUL as line terminator
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(zsplit(&output.stdout))
}

pub async fn has_unmerged_paths() -> Result<bool, Error> {
    let output = git_cmd()?
        .arg("ls-files")
        .arg("--unmerged")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

pub async fn get_diff() -> Result<Vec<u8>, Error> {
    let output = git_cmd()?
        .arg("diff")
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg("--no-textconv")
        .arg("--ignore-submodules")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(output.stdout)
}

/// Create a tree object from the current index.
///
/// The name of the new tree object is printed to standard output.
/// The index must be in a fully merged state.
pub async fn write_tree() -> Result<String, Error> {
    let output = git_cmd()?
        .arg("write-tree")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the path of the top-level directory of the working tree.
pub async fn get_root() -> Result<PathBuf, Error> {
    let output = git_cmd()?
        .arg("rev-parse")
        .arg("--show-toplevel")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

pub async fn is_dirty(path: &Path) -> Result<bool, Error> {
    let output = git_cmd()?
        .arg("diff")
        .arg("--quiet") // Implies `--exit-code`
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg(path)
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok();
    match output {
        Ok(_) => Ok(false),
        Err(err) => {
            if err
                .as_output()
                .is_some_and(|output| output.status.code() == Some(1))
            {
                Ok(true)
            } else {
                Err(err.into())
            }
        }
    }
}

async fn init_repo(url: &str, path: &Path) -> Result<(), Error> {
    git_cmd()?
        .arg("init")
        .arg("--template=")
        .arg(path)
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;

    git_cmd()?
        .current_dir(path)
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(url)
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;

    Ok(())
}

async fn shallow_clone(rev: &str, path: &Path) -> Result<(), Error> {
    git_cmd()?
        .current_dir(path)
        .arg("-c")
        .arg("protocol.version=2")
        .arg("fetch")
        .arg("origin")
        .arg(rev)
        .arg("--depth=1")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;

    git_cmd()?
        .current_dir(path)
        .arg("checkout")
        .arg("FETCH_HEAD")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;

    git_cmd()?
        .current_dir(path)
        .arg("-c")
        .arg("protocol.version=2")
        .arg("submodule")
        .arg("update")
        .arg("--init")
        .arg("--recursive")
        .arg("--depth=1")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;

    Ok(())
}

async fn full_clone(rev: &str, path: &Path) -> Result<(), Error> {
    git_cmd()?
        .current_dir(path)
        .arg("fetch")
        .arg("origin")
        .arg("--tags")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;

    git_cmd()?
        .current_dir(path)
        .arg("checkout")
        .arg(rev)
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;

    git_cmd()?
        .current_dir(path)
        .arg("submodule")
        .arg("update")
        .arg("--init")
        .arg("--recursive")
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;

    Ok(())
}

pub async fn clone_repo(url: &str, rev: &str, path: &Path) -> Result<(), Error> {
    init_repo(url, path).await?;

    if let Err(err) = shallow_clone(rev, path).await {
        warn!(?err, "Failed to shallow clone, falling back to full clone");
        full_clone(rev, path).await
    } else {
        Ok(())
    }
}

pub async fn has_hooks_path_set() -> Result<bool> {
    let output = git_cmd()?
        .arg("config")
        .arg("--get")
        .arg("core.hooksPath")
        .output()
        .await?;
    if output.status.success() {
        Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    } else {
        Ok(false)
    }
}
