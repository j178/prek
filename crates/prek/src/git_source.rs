use std::path::Path;
use std::sync::OnceLock;

use aws_lc_rs::digest::{SHA256, digest};
use prek_consts::env_vars::EnvVars;

use crate::git::{self, Error, GitCommandExt};
use crate::store::RepoMarker;

pub(crate) fn source_key(source: &str) -> String {
    hex::encode(digest(&SHA256, source.as_bytes()).as_ref())
}

/// Git 2.48 added relocatable worktrees. Older Git keeps its standalone checkout format.
pub(crate) fn supported() -> Result<bool, Error> {
    static SUPPORTED: OnceLock<bool> = OnceLock::new();
    if let Some(supported) = SUPPORTED.get() {
        return Ok(*supported);
    }

    let output = git::git_cmd()?
        .arg("--version")
        .env(EnvVars::LC_ALL, "C")
        .output_sync()?;
    let version = str::from_utf8(&output.stdout)?
        .strip_prefix("git version ")
        .unwrap_or_default();
    let mut parts = version.split('.');
    let major = parts.next().and_then(|n| n.parse::<u32>().ok());
    let minor = parts.next().and_then(|n| n.parse::<u32>().ok());
    let supported = major.zip(minor).is_some_and(|version| version >= (2, 48));
    let _ = SUPPORTED.set(supported);
    Ok(supported)
}

/// Publish a shallow checkout as a relocatable worktree. The caller holds the
/// store write lock. Only committed objects from the prepared checkout enter
/// this pool, and all network access has already finished.
pub(crate) async fn publish_shallow_worktree(
    sources: &Path,
    checkout: &Path,
    target: &Path,
    scratch: &Path,
    marker: &RepoMarker,
) -> Result<bool, Error> {
    // A full clone may have additional refs, and submodules need independent
    // repository configuration. Keep the prepared checkout in both cases.
    if !checkout.join(".git/shallow").is_file()
        || !supported()?
        || git::should_update_submodules(checkout).await?
    {
        return Ok(false);
    }
    fs_err::create_dir_all(sources)?;
    let dir = sources.join(source_key(&marker.repo));
    if !dir.try_exists()? {
        let tmp = tempfile::tempdir_in(sources)?;
        git::git_cmd()?
            .sanitize_git_repo_env()
            .args([
                "-c",
                "init.defaultObjectFormat=",
                "init",
                "--bare",
                "--quiet",
                "--template=",
            ])
            .arg(tmp.path())
            .output()
            .await?;
        // Only the main repository is bare. Its linked worktrees each have
        // their own HEAD, index and origin URL.
        for args in [
            &["config", "extensions.worktreeConfig", "true"][..],
            &["config", "--worktree", "core.bare", "true"],
            &["config", "--unset", "core.bare"],
        ] {
            git::git_cmd()?
                .current_dir(tmp.path())
                .sanitize_git_repo_env()
                .args(args)
                .output()
                .await?;
        }
        crate::fs::rename_with_retry(tmp.path(), &dir).await?;
    }

    let command = || -> Result<crate::process::Cmd, Error> {
        let mut cmd = git::git_cmd()?;
        cmd.current_dir(&dir).sanitize_git_repo_env();
        Ok(cmd)
    };
    command()?
        .args([
            "fetch",
            "--depth=1",
            "--no-tags",
            "--no-write-fetch-head",
            "--no-recurse-submodules",
        ])
        .arg(checkout)
        .arg("+HEAD:refs/prek/latest")
        .output()
        .await?;

    // Populate the worktree and its marker before moving it into repos/.
    let staging = tempfile::tempdir_in(scratch)?;
    command()?
        .args(["worktree", "add", "--relative-paths", "--quiet", "--detach"])
        .arg(staging.path())
        .arg("refs/prek/latest")
        .env(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT, "1")
        .output()
        .await?;
    git::git_cmd()?
        .current_dir(staging.path())
        .sanitize_git_repo_env()
        .args(["config", "--worktree", "remote.origin.url"])
        .hidden_args([&marker.repo])
        .output()
        .await?;
    marker.write(staging.path())?;
    command()?
        .args(["worktree", "move", "--relative-paths"])
        .arg(staging.path())
        .arg(target)
        .output()
        .await?;
    Ok(true)
}
