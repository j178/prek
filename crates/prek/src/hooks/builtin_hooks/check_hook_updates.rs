use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use clap::Parser;
use tracing::debug;

use crate::cli::auto_update::{
    find_eligible_tag, get_tag_timestamps, resolve_rev_to_commit_hash, setup_and_fetch_repo,
};
use crate::config;
use crate::hook::Hook;
use crate::store::{CacheBucket, Store};

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    /// Minimum release age (in days) required for a version to be eligible.
    /// A value of `0` disables this check.
    #[arg(long, value_name = "DAYS", default_value_t = 7)]
    cooldown_days: u8,

    /// Fail the hook if updates are available (default: warn only).
    #[arg(long, default_value_t = false)]
    fail_on_updates: bool,

    /// Minimum hours between checks (default: 24). Set to 0 to check every time.
    #[arg(long, value_name = "HOURS", default_value_t = 24)]
    check_interval_hours: u64,
}

const LAST_CHECK_FILE: &str = "hook-updates-last-check";

/// Check if configured hooks have newer versions available.
pub(crate) async fn check_hook_updates(
    hook: &Hook,
    _filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    // Check if we should skip based on check interval
    if args.check_interval_hours > 0 {
        if let Ok(store) = Store::from_settings() {
            if should_skip_check(&store, args.check_interval_hours) {
                debug!(
                    "Skipping hook update check (last check was within {} hours)",
                    args.check_interval_hours
                );
                return Ok((0, Vec::new()));
            }
        }
    }

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

    // Mark check as complete (only if we actually ran the check)
    if args.check_interval_hours > 0 {
        if let Ok(store) = Store::from_settings() {
            mark_check_complete(&store);
        }
    }

    Ok((code, output))
}

/// Check if we should skip the update check based on the last check time.
fn should_skip_check(store: &Store, interval_hours: u64) -> bool {
    let cache_file = store.cache_path(CacheBucket::Prek).join(LAST_CHECK_FILE);

    let Ok(metadata) = std::fs::metadata(&cache_file) else {
        return false;
    };

    let Ok(modified) = metadata.modified() else {
        return false;
    };

    let Ok(age) = SystemTime::now().duration_since(modified) else {
        return false;
    };

    let interval_secs = interval_hours * 3600;
    age.as_secs() < interval_secs
}

/// Mark the check as complete by touching the cache file.
fn mark_check_complete(store: &Store) {
    let cache_dir = store.cache_path(CacheBucket::Prek);
    if std::fs::create_dir_all(&cache_dir).is_err() {
        return;
    }

    let cache_file = cache_dir.join(LAST_CHECK_FILE);
    // Write current timestamp to the file
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    if let Err(e) = std::fs::write(&cache_file, now.to_string()) {
        debug!("Failed to write last check timestamp: {}", e);
    }
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

    find_eligible_tag(repo_path, &tags_with_ts, current_rev, cooldown_days).await
}

async fn resolve_head_revision(repo_path: &Path) -> Result<Option<String>> {
    resolve_rev_to_commit_hash(repo_path, "FETCH_HEAD").await
}

/// Check if two revisions point to the same commit.
async fn is_same_revision(repo_path: &Path, rev1: &str, rev2: &str) -> Result<bool> {
    let hash1 = resolve_rev_to_commit_hash(repo_path, rev1).await?;
    let hash2 = resolve_rev_to_commit_hash(repo_path, rev2).await?;

    match (hash1, hash2) {
        (Some(h1), Some(h2)) => Ok(h1 == h2),
        // If we can't resolve one of them, assume they're different
        _ => Ok(false),
    }
}
