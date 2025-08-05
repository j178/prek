use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::LazyLock;

use anyhow::Result;
use itertools::Itertools;
use tokio::io::AsyncWriteExt;
use tracing::warn;

use crate::process::Cmd;
use crate::{git, process};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Command(#[from] process::Error),
    #[error("Failed to find git: {0}")]
    GitNotFound(#[from] which::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

pub static GIT: LazyLock<Result<PathBuf, which::Error>> = LazyLock::new(|| which::which("git"));

static GIT_ENV_REMOVE: LazyLock<()> = LazyLock::new(|| {
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

    let to_remove = std::env::vars().filter(|(k, _)| {
        k.starts_with("GIT_")
            && !k.starts_with("GIT_CONFIG_KEY_")
            && !k.starts_with("GIT_CONFIG_VALUE_")
            && !keep.contains(&k.as_str())
    });

    for (k, _) in to_remove {
        unsafe { std::env::remove_var(&k) };
    }
});

pub fn git_cmd(summary: &str) -> Result<Cmd, Error> {
    let mut cmd = Cmd::new(GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?, summary);
    cmd.arg("-c").arg("core.useBuiltinFSMonitor=false");

    LazyLock::force(&GIT_ENV_REMOVE);

    Ok(cmd)
}

fn zsplit(s: &[u8]) -> Vec<String> {
    s.split(|&b| b == b'\0')
        .filter_map(|slice| {
            if slice.is_empty() {
                None
            } else {
                Some(String::from_utf8_lossy(slice).to_string())
            }
        })
        .collect()
}

pub async fn intent_to_add_files() -> Result<Vec<String>, Error> {
    let output = git_cmd("get intent to add files")?
        .arg("diff")
        .arg("--no-ext-diff")
        .arg("--ignore-submodules")
        .arg("--diff-filter=A")
        .arg("--name-only")
        .arg("-z")
        .check(true)
        .output()
        .await?;
    Ok(zsplit(&output.stdout))
}

pub async fn get_changed_files(old: &str, new: &str) -> Result<Vec<String>, Error> {
    let output = git_cmd("get changed files")?
        .arg("diff")
        .arg("--name-only")
        .arg("--diff-filter=ACMRT")
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg("-z") // Use NUL as line terminator
        .arg(format!("{old}...{new}"))
        .check(true)
        .output()
        .await?;
    Ok(zsplit(&output.stdout))
}

pub async fn git_ls_files(path: Option<&Path>) -> Result<Vec<String>, Error> {
    let mut cmd = git_cmd("get git all files")?;
    cmd.arg("ls-files").arg("-z").check(true);

    if let Some(p) = path {
        cmd.arg("--").arg(p);
    }
    let output = cmd.output().await?;
    Ok(zsplit(&output.stdout))
}

pub async fn get_git_dir() -> Result<PathBuf, Error> {
    let output = git_cmd("get git dir")?
        .arg("rev-parse")
        .arg("--git-dir")
        .check(true)
        .output()
        .await?;
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

pub async fn get_git_common_dir() -> Result<PathBuf, Error> {
    let output = git_cmd("get git common dir")?
        .arg("rev-parse")
        .arg("--git-common-dir")
        .check(true)
        .output()
        .await?;
    if output.stdout.trim_ascii().is_empty() {
        Ok(get_git_dir().await?)
    } else {
        Ok(PathBuf::from(
            String::from_utf8_lossy(&output.stdout).trim(),
        ))
    }
}

pub async fn get_staged_files() -> Result<Vec<String>, Error> {
    let output = git_cmd("get staged files")?
        .arg("diff")
        .arg("--staged")
        .arg("--name-only")
        .arg("--diff-filter=ACMRTUXB") // Everything except for D
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg("-z") // Use NUL as line terminator
        .check(true)
        .output()
        .await?;
    Ok(zsplit(&output.stdout))
}

pub async fn file_not_staged(file: &Path) -> Result<bool> {
    let status = git::git_cmd("git diff")?
        .arg("diff")
        .arg("--quiet") // Implies --exit-code
        .arg("--no-ext-diff")
        .arg(file)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .check(false)
        .status()
        .await?;

    Ok(status.code().is_some_and(|code| code == 1))
}

pub async fn has_unmerged_paths() -> Result<bool, Error> {
    let output = git_cmd("check has unmerged paths")?
        .arg("ls-files")
        .arg("--unmerged")
        .check(true)
        .output()
        .await?;
    Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
}

pub async fn is_in_merge_conflict() -> Result<bool, Error> {
    let git_dir = get_git_dir().await?;
    Ok(git_dir.join("MERGE_HEAD").try_exists()? && git_dir.join("MERGE_MSG").try_exists()?)
}

pub async fn get_conflicted_files() -> Result<Vec<String>, Error> {
    let tree = git_cmd("git write-tree")?
        .arg("write-tree")
        .check(true)
        .output()
        .await?;

    let output = git_cmd("get conflicted files")?
        .arg("diff")
        .arg("--name-only")
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg("-z") // Use NUL as line terminator
        .arg("-m")
        .arg(String::from_utf8_lossy(&tree.stdout).trim())
        .arg("HEAD")
        .arg("MERGE_HEAD")
        .check(true)
        .output()
        .await?;

    Ok(zsplit(&output.stdout)
        .into_iter()
        .chain(parse_merge_msg_for_conflicts().await?)
        .collect::<HashSet<String>>()
        .into_iter()
        .collect())
}

async fn parse_merge_msg_for_conflicts() -> Result<Vec<String>, Error> {
    let git_dir = get_git_dir().await?;
    let merge_msg = git_dir.join("MERGE_MSG");
    let content = fs_err::read_to_string(&merge_msg)?;
    let conflicts = content
        .lines()
        // Conflicted files start with tabs
        .filter(|line| line.starts_with('\t') || line.starts_with("#\t"))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .collect();
    Ok(conflicts)
}

pub async fn get_diff() -> Result<Vec<u8>, Error> {
    let output = git_cmd("git diff")?
        .arg("diff")
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg("--no-textconv")
        .arg("--ignore-submodules")
        .check(true)
        .output()
        .await?;
    Ok(output.stdout)
}

/// Create a tree object from the current index.
///
/// The name of the new tree object is printed to standard output.
/// The index must be in a fully merged state.
pub async fn write_tree() -> Result<String, Error> {
    let output = git_cmd("git write-tree")?
        .arg("write-tree")
        .check(true)
        .output()
        .await?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Get the path of the top-level directory of the working tree.
pub async fn get_root() -> Result<PathBuf, Error> {
    let output = git_cmd("get git root")?
        .arg("rev-parse")
        .arg("--show-toplevel")
        .check(true)
        .output()
        .await?;
    Ok(PathBuf::from(
        String::from_utf8_lossy(&output.stdout).trim(),
    ))
}

pub async fn is_dirty(path: &Path) -> Result<bool, Error> {
    let mut cmd = git_cmd("check git is dirty")?;
    let output = cmd
        .arg("diff")
        .arg("--quiet") // Implies `--exit-code`
        .arg("--no-ext-diff") // Disable external diff drivers
        .arg(path)
        .check(false)
        .output()
        .await?;
    if output.status.success() {
        Ok(false)
    } else if output.status.code() == Some(1) {
        Ok(true)
    } else {
        Err(cmd.check_status(output.status).unwrap_err().into())
    }
}

async fn init_repo(url: &str, path: &Path) -> Result<(), Error> {
    git_cmd("init git repo")?
        .arg("init")
        .arg("--template=")
        .arg(path)
        .check(true)
        .output()
        .await?;

    git_cmd("add git remote")?
        .current_dir(path)
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(url)
        .check(true)
        .output()
        .await?;

    Ok(())
}

async fn shallow_clone(rev: &str, path: &Path) -> Result<(), Error> {
    git_cmd("git shallow clone")?
        .current_dir(path)
        .arg("-c")
        .arg("protocol.version=2")
        .arg("fetch")
        .arg("origin")
        .arg(rev)
        .arg("--depth=1")
        .check(true)
        .output()
        .await?;

    git_cmd("git checkout")?
        .current_dir(path)
        .arg("checkout")
        .arg("FETCH_HEAD")
        .check(true)
        .output()
        .await?;

    git_cmd("update git submodules")?
        .current_dir(path)
        .arg("-c")
        .arg("protocol.version=2")
        .arg("submodule")
        .arg("update")
        .arg("--init")
        .arg("--recursive")
        .arg("--depth=1")
        .check(true)
        .output()
        .await?;

    Ok(())
}

async fn full_clone(rev: &str, path: &Path) -> Result<(), Error> {
    git_cmd("git full clone")?
        .current_dir(path)
        .arg("fetch")
        .arg("origin")
        .arg("--tags")
        .check(true)
        .output()
        .await?;

    git_cmd("git checkout")?
        .current_dir(path)
        .arg("checkout")
        .arg(rev)
        .check(true)
        .output()
        .await?;

    git_cmd("update git submodules")?
        .current_dir(path)
        .arg("submodule")
        .arg("update")
        .arg("--init")
        .arg("--recursive")
        .check(true)
        .output()
        .await?;

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
    let output = git_cmd("get git hooks path")?
        .arg("config")
        .arg("--get")
        .arg("core.hooksPath")
        .check(false)
        .output()
        .await?;
    if output.status.success() {
        Ok(!String::from_utf8_lossy(&output.stdout).trim().is_empty())
    } else {
        Ok(false)
    }
}

pub async fn lfs_files<T: FromIterator<String>>(paths: &[&String]) -> Result<T, Error> {
    let mut job = git_cmd("git check-attr")?
        .arg("check-attr")
        .arg("filter")
        .arg("-z")
        .arg("--stdin")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .check(true)
        // .output()
        .spawn()?;

    {
        let mut stdin = job.stdin.take().expect("Failed to open stdin");
        stdin.write_all(paths.iter().join("\0").as_ref()).await?;
    }

    Ok(
        String::from_utf8_lossy(&job.wait_with_output().await?.stdout)
            .trim()
            .split('\0')
            .tuples::<(_, _, _)>()
            .filter_map(|(file, _, attr)| {
                if attr == "lfs" {
                    Some(file.to_owned())
                } else {
                    None
                }
            })
            .collect(),
    )
}
