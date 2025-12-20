use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::Parser;

use crate::cli::auto_update::{get_best_candidate_tag, get_tag_timestamps, setup_and_fetch_repo};
use crate::config;
use crate::git::git_cmd;
use crate::hook::Hook;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    /// Minimum release age (in days) required for a version to be eligible.
    /// A value of `0` disables this check.
    #[arg(long, value_name = "DAYS", default_value_t = 0)]
    cooldown_days: u8,

    /// Fail the hook if updates are available (default: warn only).
    #[arg(long, default_value_t = false)]
    fail_on_updates: bool,
}

/// Check if configured hooks have newer versions available.
pub(crate) async fn check_hook_updates(
    hook: &Hook,
    _filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    let project_config = hook.project().config();

    let mut code = 0;
    let mut output = Vec::new();

    for repo in &project_config.repos {
        if let config::Repo::Remote(remote_repo) = repo {
            match check_repo_for_updates(remote_repo, args.cooldown_days).await {
                Ok(Some(update_info)) => {
                    writeln!(
                        &mut output,
                        "{}: {} -> {} available",
                        remote_repo.repo, remote_repo.rev, update_info.new_rev
                    )?;
                    if args.fail_on_updates {
                        code = 1;
                    }
                }
                Ok(None) => {
                    // No update available or already up to date
                }
                Err(e) => {
                    writeln!(
                        &mut output,
                        "{}: failed to check for updates: {}",
                        remote_repo.repo, e
                    )?;
                    // Don't fail on network errors, just warn
                }
            }
        }
    }

    Ok((code, output))
}

struct UpdateInfo {
    new_rev: String,
}

async fn check_repo_for_updates(
    repo: &config::RemoteRepo,
    cooldown_days: u8,
) -> Result<Option<UpdateInfo>> {
    let tmp_dir = tempfile::tempdir()?;
    let repo_path = tmp_dir.path();

    // Initialize and fetch the repo (lightweight fetch, tags only)
    setup_and_fetch_repo(&repo.repo, repo_path).await?;

    // Get the latest eligible revision
    let latest_rev = resolve_latest_revision(repo_path, &repo.rev, cooldown_days).await?;

    let Some(latest_rev) = latest_rev else {
        return Ok(None);
    };

    // Check if the latest revision is different from the current one
    if is_same_revision(repo_path, &repo.rev, &latest_rev).await? {
        return Ok(None);
    }

    Ok(Some(UpdateInfo {
        new_rev: latest_rev,
    }))
}

async fn resolve_latest_revision(
    repo_path: &Path,
    current_rev: &str,
    cooldown_days: u8,
) -> Result<Option<String>> {
    let tags_with_ts = get_tag_timestamps(repo_path).await?;

    if tags_with_ts.is_empty() {
        // No tags, try to get the latest commit from HEAD
        return resolve_head_revision(repo_path).await;
    }

    let cutoff_secs = u64::from(cooldown_days) * 86400;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let cutoff = now.saturating_sub(cutoff_secs);

    // tags_with_ts is sorted newest -> oldest; find the first bucket where ts <= cutoff.
    let left = match tags_with_ts.binary_search_by(|(_, ts)| ts.cmp(&cutoff).reverse()) {
        Ok(i) | Err(i) => i,
    };

    let Some((target_tag, _)) = tags_with_ts.get(left) else {
        // All tags are too new
        return Ok(None);
    };

    // Try to find the best candidate tag (prefer version-like tags)
    let best = get_best_candidate_tag(repo_path, target_tag, current_rev)
        .await
        .unwrap_or_else(|_| target_tag.clone());

    Ok(Some(best))
}

async fn resolve_head_revision(repo_path: &Path) -> Result<Option<String>> {
    let output = git_cmd("git rev-parse")?
        .arg("rev-parse")
        .arg("FETCH_HEAD")
        .check(false)
        .current_dir(repo_path)
        .output()
        .await?;

    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    } else {
        Ok(None)
    }
}

/// Check if two revisions point to the same commit.
async fn is_same_revision(repo_path: &Path, rev1: &str, rev2: &str) -> Result<bool> {
    let hash1 = resolve_to_hash(repo_path, rev1).await?;
    let hash2 = resolve_to_hash(repo_path, rev2).await?;

    match (hash1, hash2) {
        (Some(h1), Some(h2)) => Ok(h1 == h2),
        // If we can't resolve one of them, assume they're different
        _ => Ok(false),
    }
}

async fn resolve_to_hash(repo_path: &Path, rev: &str) -> Result<Option<String>> {
    let output = git_cmd("git rev-parse")?
        .arg("rev-parse")
        .arg(format!("{rev}^{{}}"))
        .check(false)
        .current_dir(repo_path)
        .output()
        .await?;

    if output.status.success() {
        Ok(Some(
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ))
    } else {
        Ok(None)
    }
}
