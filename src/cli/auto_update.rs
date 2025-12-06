use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use bstr::ByteSlice;
use futures::StreamExt;
use itertools::Itertools;
use lazy_regex::regex;
use owo_colors::OwoColorize;
use prek_consts::MANIFEST_FILE;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use serde::Serializer;
use serde::ser::SerializeMap;
use tracing::trace;

use crate::cli::ExitStatus;
use crate::cli::reporter::AutoUpdateReporter;
use crate::config::{RemoteRepo, Repo};
use crate::fs::CWD;
use crate::printer::Printer;
use crate::run::CONCURRENCY;
use crate::store::Store;
use crate::workspace::{Project, Workspace};
use crate::{config, git};

#[derive(Default, Clone)]
struct Revision {
    rev: String,
    frozen: Option<String>,
}

pub(crate) async fn auto_update(
    store: &Store,
    config: Option<PathBuf>,
    filter_repos: Vec<String>,
    bleeding_edge: bool,
    freeze: bool,
    jobs: usize,
    dry_run: bool,
    cooldown_days: u8,
    printer: Printer,
) -> Result<ExitStatus> {
    struct RepoInfo<'a> {
        project: &'a Project,
        remote_size: usize,
        remote_index: usize,
    }

    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    // TODO: support selectors?
    let workspace = Workspace::discover(store, workspace_root, config, None, true)?;

    // Collect repos and deduplicate by RemoteRepo
    #[allow(clippy::mutable_key_type)]
    let mut repo_updates: FxHashMap<&RemoteRepo, Vec<RepoInfo>> = FxHashMap::default();

    for project in workspace.projects() {
        let remote_size = project
            .config()
            .repos
            .iter()
            .filter(|r| matches!(r, Repo::Remote(_)))
            .count();

        let mut remote_index = 0;
        for repo in &project.config().repos {
            if let Repo::Remote(remote_repo) = repo {
                let updates = repo_updates.entry(remote_repo).or_default();
                updates.push(RepoInfo {
                    project,
                    remote_size,
                    remote_index,
                });
                remote_index += 1;
            }
        }
    }

    let jobs = if jobs == 0 { *CONCURRENCY } else { jobs };
    let jobs = jobs
        .min(if filter_repos.is_empty() {
            repo_updates.len()
        } else {
            filter_repos.len()
        })
        .max(1);

    let reporter = AutoUpdateReporter::from(printer);

    let mut tasks = futures::stream::iter(repo_updates.iter().filter(|(remote_repo, _)| {
        // Filter by user specified repositories
        if filter_repos.is_empty() {
            true
        } else {
            filter_repos.iter().any(|r| r == remote_repo.repo.as_str())
        }
    }))
    .map(async |(remote_repo, _)| {
        let progress = reporter.on_update_start(&remote_repo.to_string());

        let result = update_repo(remote_repo, bleeding_edge, freeze, cooldown_days).await;

        reporter.on_update_complete(progress);

        (*remote_repo, result)
    })
    .buffer_unordered(jobs)
    .collect::<Vec<_>>()
    .await;

    // Sort tasks by repository URL for consistent output order
    tasks.sort_by(|(a, _), (b, _)| a.repo.cmp(&b.repo));

    reporter.on_complete();

    // Group results by project config file
    #[allow(clippy::mutable_key_type)]
    let mut project_updates: FxHashMap<&Project, Vec<Option<Revision>>> = FxHashMap::default();
    let mut failure = false;

    for (remote_repo, result) in tasks {
        match result {
            Ok(new_rev) => {
                if remote_repo.rev == new_rev.rev {
                    writeln!(
                        printer.stdout(),
                        "[{}] already up to date",
                        remote_repo.repo.as_str().yellow()
                    )?;
                } else {
                    writeln!(
                        printer.stdout(),
                        "[{}] updating {} -> {}",
                        remote_repo.repo.as_str().cyan(),
                        remote_repo.rev,
                        new_rev.rev
                    )?;
                }

                // Apply this update to all projects that reference this repo
                if let Some(projects) = repo_updates.get(&remote_repo) {
                    for RepoInfo {
                        project,
                        remote_size,
                        remote_index,
                    } in projects
                    {
                        let revisions = project_updates
                            .entry(project)
                            .or_insert_with(|| vec![None; *remote_size]);
                        revisions[*remote_index] = Some(new_rev.clone());
                    }
                }
            }
            Err(e) => {
                failure = true;
                writeln!(
                    printer.stderr(),
                    "[{}] update failed: {e}",
                    remote_repo.repo.as_str().red()
                )?;
            }
        }
    }

    if !dry_run {
        // Update each project config file
        for (project, revisions) in project_updates {
            let has_changes = revisions.iter().any(Option::is_some);
            if has_changes {
                write_new_config(project.config_file(), &revisions).await?;
            }
        }
    }

    if failure {
        return Ok(ExitStatus::Failure);
    }
    Ok(ExitStatus::Success)
}

async fn update_repo(
    repo: &RemoteRepo,
    bleeding_edge: bool,
    freeze: bool,
    cooldown_days: u8,
) -> Result<Revision> {
    let tmp_dir = tempfile::tempdir()?;

    trace!(
        "Cloning repository `{}` to `{}`",
        repo.repo,
        tmp_dir.path().display()
    );

    setup_and_fetch_repo(repo.repo.as_str(), tmp_dir.path()).await?;

    let rev = resolve_revision(tmp_dir.path(), &repo.rev, bleeding_edge, cooldown_days).await?;

    let Some(rev) = rev else {
        // All tags filtered by cooldown - no update available
        return Ok(Revision {
            rev: repo.rev.clone(),
            frozen: None,
        });
    };

    let (rev, frozen) = maybe_freeze_revision(tmp_dir.path(), rev, freeze).await?;

    checkout_and_validate_manifest(tmp_dir.path(), &rev, repo).await?;

    let new_revision = Revision { rev, frozen };

    Ok(new_revision)
}

async fn setup_and_fetch_repo(repo_url: &str, repo_path: &Path) -> Result<()> {
    git::init_repo(repo_url, repo_path).await?;
    git::git_cmd("git config")?
        .arg("config")
        .arg("extensions.partialClone")
        .arg("true")
        .current_dir(repo_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    git::git_cmd("git fetch")?
        .arg("fetch")
        .arg("origin")
        .arg("HEAD")
        .arg("--quiet")
        .arg("--filter=blob:none")
        .arg("--tags")
        .current_dir(repo_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    Ok(())
}

async fn resolve_revision(
    repo_path: &Path,
    current_rev: &str,
    bleeding_edge: bool,
    cooldown_days: u8,
) -> Result<Option<String>> {
    let mut cmd = git::git_cmd("git describe")?;
    cmd.arg("describe")
        .arg("FETCH_HEAD")
        .arg("--tags") // use any tags found in refs/tags
        .check(false)
        .current_dir(repo_path);
    if bleeding_edge {
        cmd.arg("--exact-match")
    } else {
        // `--abbrev=0` suppress long format, find the closest tag name without any suffix
        cmd.arg("--abbrev=0")
    };

    let output = cmd.output().await?;
    let rev = if output.status.success() {
        let rev = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if let Some(best) =
            get_best_candidate_tag(repo_path, &rev, current_rev, cooldown_days).await?
        {
            trace!("Using best candidate tag `{best}`");
            best
        } else {
            // All tags filtered by cooldown - no update available
            trace!("No candidate tags found");
            return Ok(None);
        }
    } else {
        trace!("Failed to describe FETCH_HEAD, using rev-parse instead");
        // "fatal: no tag exactly matches xxx"
        let stdout = git::git_cmd("git rev-parse")?
            .arg("rev-parse")
            .arg("FETCH_HEAD")
            .check(true)
            .current_dir(repo_path)
            .output()
            .await?
            .stdout;
        String::from_utf8_lossy(&stdout).trim().to_string()
    };
    trace!("Resolved latest tag to `{rev}`");

    Ok(Some(rev))
}

async fn maybe_freeze_revision(
    repo_path: &Path,
    mut rev: String,
    freeze: bool,
) -> Result<(String, Option<String>)> {
    let mut frozen = None;
    if freeze {
        let exact = git::git_cmd("git rev-parse")?
            .arg("rev-parse")
            .arg(&rev)
            .current_dir(repo_path)
            .output()
            .await?
            .stdout;
        let exact = String::from_utf8_lossy(&exact).trim().to_string();
        if rev != exact {
            trace!("Freezing revision to `{exact}`");
            frozen = Some(rev);
            rev = exact;
        }
    }

    Ok((rev, frozen))
}

async fn checkout_and_validate_manifest(
    repo_path: &Path,
    rev: &str,
    repo: &RemoteRepo,
) -> Result<()> {
    // Workaround for Windows: https://github.com/pre-commit/pre-commit/issues/2865,
    // https://github.com/j178/prek/issues/614
    if cfg!(windows) {
        git::git_cmd("git show")?
            .arg("show")
            .arg(format!("{rev}:{MANIFEST_FILE}"))
            .current_dir(repo_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
    }

    git::git_cmd("git checkout")?
        .arg("checkout")
        .arg("--quiet")
        .arg(rev)
        .arg("--")
        .arg(MANIFEST_FILE)
        .current_dir(repo_path)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    let manifest = config::read_manifest(&repo_path.join(MANIFEST_FILE))?;
    let new_hook_ids = manifest
        .hooks
        .into_iter()
        .map(|h| h.id)
        .collect::<FxHashSet<_>>();
    let hooks_missing = repo
        .hooks
        .iter()
        .filter(|h| !new_hook_ids.contains(&h.id))
        .map(|h| h.id.clone())
        .collect::<Vec<_>>();
    if !hooks_missing.is_empty() {
        return Err(anyhow::anyhow!(
            "Cannot update to rev `{}`, hook{} {} missing: {}",
            rev,
            if hooks_missing.len() > 1 { "s" } else { "" },
            if hooks_missing.len() > 1 { "are" } else { "is" },
            hooks_missing.join(", ")
        ));
    }

    Ok(())
}

/// Returns a map of tag names to their Unix timestamps, fetched in a single git command.
async fn get_all_tag_timestamps(repo: &Path) -> Result<FxHashMap<String, u64>> {
    let output = git::git_cmd("git log")?
        .arg("log")
        .arg("--tags")
        .arg("--format=%ct %D")
        .arg("--simplify-by-decoration")
        .current_dir(repo)
        .output()
        .await?;

    let mut timestamps = FxHashMap::default();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let Some((timestamp_str, refs)) = line.split_once(' ') else {
            continue;
        };
        let Ok(timestamp) = timestamp_str.parse::<u64>() else {
            continue;
        };
        for tag_ref in refs.split(", ") {
            if let Some(tag_name) = tag_ref.strip_prefix("tag: ") {
                timestamps.insert(tag_name.to_string(), timestamp);
            }
        }
    }

    Ok(timestamps)
}

/// Filters tags to only those older than the cooldown period.
fn filter_tags_by_cooldown<'a>(
    tags: &[&'a str],
    cooldown_days: u8,
    timestamps: &FxHashMap<String, u64>,
) -> Vec<&'a str> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let cutoff_secs = u64::from(cooldown_days) * 86400;

    let mut result = Vec::new();
    for tag in tags {
        if let Some(&timestamp) = timestamps.get(*tag) {
            let age_secs = now.saturating_sub(timestamp);
            if age_secs >= cutoff_secs {
                result.push(*tag);
            } else {
                trace!(
                    "Skipping tag `{tag}` - only {} day(s) old, cooldown is {cooldown_days}",
                    age_secs / 86400
                );
            }
        }
    }
    result
}

/// Returns the best candidate tag, or None if no suitable tag is found
/// (when all tags are filtered out by the cooldown period).
///
/// Multiple tags can exist on an SHA. Sometimes a moving tag is attached
/// to a version tag. Try to pick the tag that looks like a version and most similar
/// to the current revision, using the Levenshtein string edit distance of the SHAs.
async fn get_best_candidate_tag(
    repo: &Path,
    rev: &str,
    current_rev: &str,
    cooldown_days: u8,
) -> Result<Option<String>> {
    // First, try tags pointing at this specific commit
    let stdout = git::git_cmd("git tag")?
        .arg("tag")
        .arg("--points-at")
        .arg(format!("{rev}^{{}}"))
        .check(true)
        .current_dir(repo)
        .output()
        .await?
        .stdout;

    let stdout_str = String::from_utf8_lossy(&stdout).into_owned();
    let tags: Vec<&str> = stdout_str
        .lines()
        .filter(|line| line.contains('.'))
        .collect();

    if tags.is_empty() {
        return Err(anyhow::anyhow!("No tags found for revision {rev}"));
    }

    // Fetch all timestamps once upfront
    let timestamps = get_all_tag_timestamps(repo).await?;

    let candidates = if cooldown_days > 0 {
        let filtered = filter_tags_by_cooldown(&tags, cooldown_days, &timestamps);
        if filtered.is_empty() {
            // No tags on this commit satisfy cooldown - search ALL tags
            trace!("No tags on {rev} satisfy cooldown, searching all tags");
            return find_best_tag_with_cooldown(repo, current_rev, cooldown_days, &timestamps)
                .await;
        }
        filtered
    } else {
        tags
    };

    Ok(candidates
        .into_iter()
        .sorted_by_key(|tag| {
            // Prefer tags that are more similar to the current revision
            levenshtein::levenshtein(tag, current_rev)
        })
        .next()
        .map(ToString::to_string))
}

/// Search all tags in the repo, filter by cooldown, and return the newest one by commit time.
async fn find_best_tag_with_cooldown(
    repo: &Path,
    current_rev: &str,
    cooldown_days: u8,
    timestamps: &FxHashMap<String, u64>,
) -> Result<Option<String>> {
    // Get all version-like tags
    let stdout = git::git_cmd("git tag")?
        .arg("tag")
        .check(true)
        .current_dir(repo)
        .output()
        .await?
        .stdout;

    let stdout_str = String::from_utf8_lossy(&stdout).into_owned();
    let all_tags: Vec<&str> = stdout_str
        .lines()
        .filter(|line| line.contains('.'))
        .collect();

    if all_tags.is_empty() {
        return Ok(None);
    }

    let candidates = filter_tags_by_cooldown(&all_tags, cooldown_days, timestamps);

    if candidates.is_empty() {
        trace!("No tags in the repository satisfy cooldown of {cooldown_days} day(s)");
        return Ok(None);
    }

    // Find the tag with the newest commit timestamp
    let mut best: Option<(&str, u64)> = None;
    for tag in candidates {
        if let Some(&timestamp) = timestamps.get(tag) {
            match &best {
                None => best = Some((tag, timestamp)),
                Some((_best_tag, best_ts)) if timestamp > *best_ts => {
                    best = Some((tag, timestamp));
                }
                Some((best_tag, best_ts)) if timestamp == *best_ts => {
                    // Same timestamp - use Levenshtein as tiebreaker
                    if levenshtein::levenshtein(tag, current_rev)
                        < levenshtein::levenshtein(best_tag, current_rev)
                    {
                        best = Some((tag, timestamp));
                    }
                }
                _ => {}
            }
        }
    }

    Ok(best.map(|(tag, _)| tag.to_string()))
}

async fn write_new_config(path: &Path, revisions: &[Option<Revision>]) -> Result<()> {
    let mut lines = fs_err::tokio::read_to_string(path)
        .await?
        .split_inclusive('\n')
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    let rev_regex = regex!(r#"^(\s+)rev:(\s*)(['"]?)([^\s#]+)(.*)(\r?\n)$"#);

    let rev_lines = lines
        .iter()
        .enumerate()
        .filter_map(|(line_no, line)| {
            if rev_regex.is_match(line) {
                Some(line_no)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if rev_lines.len() != revisions.len() {
        anyhow::bail!(
            "Found {} `rev:` lines in `{}` but expected {}, file content may have changed",
            rev_lines.len(),
            path.display(),
            revisions.len()
        );
    }

    for (line_no, revision) in rev_lines.iter().zip_eq(revisions) {
        let Some(revision) = revision else {
            // This repo was not updated, skip
            continue;
        };

        let mut new_rev = Vec::new();
        let mut serializer = serde_yaml::Serializer::new(&mut new_rev);
        serializer
            .serialize_map(Some(1))?
            .serialize_entry("rev", &revision.rev)?;
        serializer.end()?;

        let (_, new_rev) = new_rev
            .to_str()?
            .split_once(':')
            .expect("Failed to split serialized revision");

        let caps = rev_regex
            .captures(&lines[*line_no])
            .context("Failed to capture rev line")?;

        let comment = if let Some(frozen) = &revision.frozen {
            format!("  # frozen: {frozen}")
        } else if caps[5].trim().starts_with("# frozen:") {
            String::new()
        } else {
            caps[5].to_string()
        };

        lines[*line_no] = format!(
            "{}rev:{}{}{}{}",
            &caps[1],
            &caps[2],
            new_rev.trim(),
            comment,
            &caps[6]
        );
    }

    fs_err::tokio::write(path, lines.join("").as_bytes())
        .await
        .with_context(|| format!("Failed to write updated config file `{}`", path.display()))?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    async fn setup_test_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();

        // Initialize git repo
        git::git_cmd("git init")
            .unwrap()
            .arg("init")
            .current_dir(repo)
            .output()
            .await
            .unwrap();

        // Configure git user
        git::git_cmd("git config")
            .unwrap()
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo)
            .output()
            .await
            .unwrap();

        git::git_cmd("git config")
            .unwrap()
            .args(["config", "user.name", "Test"])
            .current_dir(repo)
            .output()
            .await
            .unwrap();

        // First commit (required before creating a branch)
        git::git_cmd("git commit")
            .unwrap()
            .args(["commit", "--allow-empty", "-m", "initial"])
            .current_dir(repo)
            .output()
            .await
            .unwrap();

        // Create a trunk branch (avoid dangling commits)
        git::git_cmd("git checkout")
            .unwrap()
            .args(["branch", "-M", "trunk"])
            .current_dir(repo)
            .output()
            .await
            .unwrap();

        tmp
    }

    async fn create_commit(repo: &Path, message: &str) {
        git::git_cmd("git commit")
            .unwrap()
            .args(["commit", "--allow-empty", "-m", message])
            .current_dir(repo)
            .output()
            .await
            .unwrap();
    }

    async fn create_backdated_commit(repo: &Path, message: &str, days_ago: u64) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - (days_ago * 86400);

        let date_str = format!("{timestamp} +0000");

        git::git_cmd("git commit")
            .unwrap()
            .args(["commit", "--allow-empty", "-m", message])
            .env("GIT_AUTHOR_DATE", &date_str)
            .env("GIT_COMMITTER_DATE", &date_str)
            .current_dir(repo)
            .output()
            .await
            .unwrap();
    }

    async fn create_tag(repo: &Path, tag: &str) {
        git::git_cmd("git tag")
            .unwrap()
            .args(["tag", tag])
            .current_dir(repo)
            .output()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_get_all_tag_timestamps() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_commit(repo, "initial").await;
        create_tag(repo, "v1.0.0").await;

        let timestamps = get_all_tag_timestamps(repo).await.unwrap();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();

        assert!(timestamps.contains_key("v1.0.0"));
        // Tag should be created within the last minute
        assert!(now - timestamps["v1.0.0"] < 60);
    }

    #[tokio::test]
    async fn test_filter_tags_by_cooldown_all_pass() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "initial", 5).await;
        create_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "second", 3).await;
        create_tag(repo, "v1.1.0").await;

        let timestamps = get_all_tag_timestamps(repo).await.unwrap();
        let tags = vec!["v1.0.0", "v1.1.0"];
        let filtered = filter_tags_by_cooldown(&tags, 2, &timestamps);

        assert_eq!(filtered.len(), 2);
        assert!(filtered.contains(&"v1.0.0"));
        assert!(filtered.contains(&"v1.1.0"));
    }

    #[tokio::test]
    async fn test_filter_tags_by_cooldown_some_filtered() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "initial", 5).await;
        create_tag(repo, "v1.0.0").await;

        create_commit(repo, "second").await; // Created now, should be filtered
        create_tag(repo, "v1.1.0").await;

        let timestamps = get_all_tag_timestamps(repo).await.unwrap();
        let tags = vec!["v1.0.0", "v1.1.0"];
        let filtered = filter_tags_by_cooldown(&tags, 2, &timestamps);

        assert_eq!(filtered.len(), 1);
        assert!(filtered.contains(&"v1.0.0"));
    }

    #[tokio::test]
    async fn test_filter_tags_by_cooldown_all_filtered() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_commit(repo, "initial").await;
        create_tag(repo, "v1.0.0").await;

        let timestamps = get_all_tag_timestamps(repo).await.unwrap();
        let tags = vec!["v1.0.0"];
        let filtered = filter_tags_by_cooldown(&tags, 2, &timestamps);

        assert!(filtered.is_empty());
    }

    #[tokio::test]
    async fn test_filter_tags_by_cooldown_zero_disables() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_commit(repo, "initial").await;
        create_tag(repo, "v1.0.0").await;

        let timestamps = get_all_tag_timestamps(repo).await.unwrap();
        let tags = vec!["v1.0.0"];
        let filtered = filter_tags_by_cooldown(&tags, 0, &timestamps);

        // With 0 days cooldown, age >= 0 is always true
        assert_eq!(filtered.len(), 1);
    }

    #[tokio::test]
    async fn test_cooldown_should_fallback_to_older_tag_on_different_commit() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        // Create a commit and tag from 10 days ago
        create_backdated_commit(repo, "first release", 10).await;
        create_tag(repo, "v1.8.0").await;

        // Create a commit and tag from 5 days ago
        create_backdated_commit(repo, "second release", 5).await;
        create_tag(repo, "v1.9.0").await;

        // Create a commit and tag from 1 day ago (too new for cooldown)
        create_backdated_commit(repo, "third release", 1).await;
        create_tag(repo, "v2.0.0").await;

        let result = get_best_candidate_tag(repo, "v2.0.0", "v1.8.0", 3)
            .await
            .unwrap();

        assert_eq!(
            result,
            Some("v1.9.0".to_string()),
            "Should fallback to v1.9.0 (newest tag satisfying cooldown), not None"
        );
    }
}
