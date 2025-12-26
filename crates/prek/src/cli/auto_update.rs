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
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use serde::Serializer;
use serde::ser::SerializeMap;
use tracing::{debug, trace};

use crate::cli::ExitStatus;
use crate::cli::reporter::AutoUpdateReporter;
use crate::cli::run::Selectors;
use crate::config::{RemoteRepo, Repo};
use crate::fs::{CWD, Simplified};
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
    let selectors = Selectors::default();
    let workspace = Workspace::discover(store, workspace_root, config, Some(&selectors), true)?;

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
    let repo_path = tmp_dir.path();

    trace!(
        "Cloning repository `{}` to `{}`",
        repo.repo,
        repo_path.display()
    );

    setup_and_fetch_repo(repo.repo.as_str(), repo_path).await?;

    let rev = resolve_revision(repo_path, &repo.rev, bleeding_edge, cooldown_days).await?;

    let Some(rev) = rev else {
        debug!("No suitable revision found for repo `{}`", repo.repo);
        return Ok(Revision {
            rev: repo.rev.clone(),
            frozen: None,
        });
    };

    let (rev, frozen) = if freeze && let Some(exact) = freeze_revision(repo_path, &rev).await? {
        debug!("Freezing revision `{rev}` to `{exact}`");
        (exact, Some(rev))
    } else {
        (rev, None)
    };

    checkout_and_validate_manifest(repo_path, &rev, repo).await?;

    Ok(Revision { rev, frozen })
}

async fn setup_and_fetch_repo(repo_url: &str, repo_path: &Path) -> Result<()> {
    git::init_repo(repo_url, repo_path).await?;
    git::git_cmd("git config")?
        .arg("config")
        .arg("extensions.partialClone")
        .arg("true")
        .current_dir(repo_path)
        .remove_git_envs()
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
        .remove_git_envs()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    Ok(())
}

async fn resolve_bleeding_edge(repo_path: &Path) -> Result<Option<String>> {
    let output = git::git_cmd("git describe")?
        .arg("describe")
        .arg("FETCH_HEAD")
        // Instead of using only the annotated tags, use any tag found in refs/tags namespace.
        // This option enables matching a lightweight (non-annotated) tag.
        .arg("--tags")
        // Only output exact matches (a tag directly references the supplied commit).
        // This is a synonym for --candidates=0.
        .arg("--exact-match")
        .check(false)
        .current_dir(repo_path)
        .remove_git_envs()
        .output()
        .await?;
    let rev = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        debug!("No matching tag for `FETCH_HEAD`, using rev-parse instead");
        // "fatal: no tag exactly matches xxx"
        let output = git::git_cmd("git rev-parse")?
            .arg("rev-parse")
            .arg("FETCH_HEAD")
            .check(true)
            .current_dir(repo_path)
            .remove_git_envs()
            .output()
            .await?;
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    };

    debug!("Resolved `FETCH_HEAD` to `{rev}`");
    Ok(Some(rev))
}

/// Returns all tags and their Unix timestamps (newest first).
async fn get_tag_timestamps(repo: &Path) -> Result<Vec<(String, u64)>> {
    let output = git::git_cmd("git for-each-ref")?
        .arg("for-each-ref")
        .arg("--sort=-creatordate")
        // `creatordate` is the date the tag was created (annotated tags) or the commit date (lightweight tags)
        // `lstrip=2` removes the "refs/tags/" prefix
        .arg("--format=%(refname:lstrip=2) %(creatordate:unix)")
        .arg("refs/tags")
        .check(true)
        .current_dir(repo)
        .remove_git_envs()
        .output()
        .await?;

    Ok(String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split_whitespace();
            let tag = parts.next()?.trim_ascii();
            let ts_str = parts.next()?.trim_ascii();
            let ts: u64 = ts_str.parse().ok()?;
            Some((tag.to_string(), ts))
        })
        .collect())
}

async fn resolve_revision(
    repo_path: &Path,
    current_rev: &str,
    bleeding_edge: bool,
    cooldown_days: u8,
) -> Result<Option<String>> {
    if bleeding_edge {
        return resolve_bleeding_edge(repo_path).await;
    }

    let tags_with_ts = get_tag_timestamps(repo_path).await?;

    let cutoff_secs = u64::from(cooldown_days) * 86400;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let cutoff = now.saturating_sub(cutoff_secs);

    // tags_with_ts is sorted newest -> oldest; find the first bucket where ts <= cutoff.
    let left = match tags_with_ts.binary_search_by(|(_, ts)| ts.cmp(&cutoff).reverse()) {
        Ok(i) | Err(i) => i,
    };

    let Some((target_tag, target_ts)) = tags_with_ts.get(left) else {
        trace!("No tags meet cooldown cutoff {cutoff_secs}s");
        return Ok(None);
    };

    debug!("Using tag `{target_tag}` cutoff timestamp {target_ts}");

    let best = get_best_candidate_tag(repo_path, target_tag, current_rev)
        .await
        .unwrap_or_else(|_| target_tag.clone());
    debug!("Using best candidate tag `{best}` for revision `{target_tag}`");

    Ok(Some(best))
}

async fn freeze_revision(repo_path: &Path, rev: &str) -> Result<Option<String>> {
    let exact = git::git_cmd("git rev-parse")?
        .arg("rev-parse")
        .arg(format!("{rev}^{{}}"))
        .current_dir(repo_path)
        .remove_git_envs()
        .output()
        .await?
        .stdout;
    let exact = str::from_utf8(&exact)?.trim();
    if rev == exact {
        Ok(None)
    } else {
        Ok(Some(exact.to_string()))
    }
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
            .arg(format!("{rev}:{PRE_COMMIT_HOOKS_YAML}"))
            .current_dir(repo_path)
            .remove_git_envs()
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
        .arg(PRE_COMMIT_HOOKS_YAML)
        .current_dir(repo_path)
        .remove_git_envs()
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    let manifest = config::read_manifest(&repo_path.join(PRE_COMMIT_HOOKS_YAML))?;
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

/// Multiple tags can exist on an SHA. Sometimes a moving tag is attached
/// to a version tag. Try to pick the tag that looks like a version and most similar
/// to the current revision.
async fn get_best_candidate_tag(repo: &Path, rev: &str, current_rev: &str) -> Result<String> {
    let stdout = git::git_cmd("git tag")?
        .arg("tag")
        .arg("--points-at")
        .arg(format!("{rev}^{{}}"))
        .check(true)
        .current_dir(repo)
        .remove_git_envs()
        .output()
        .await?
        .stdout;

    String::from_utf8_lossy(&stdout)
        .lines()
        .filter(|line| line.contains('.'))
        .sorted_by_key(|tag| {
            // Prefer tags that are more similar to the current revision
            levenshtein::levenshtein(tag, current_rev)
        })
        .next()
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("No tags found for revision {rev}"))
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
            path.user_display(),
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
        .with_context(|| {
            format!(
                "Failed to write updated config file `{}`",
                path.user_display()
            )
        })?;

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
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        // Configure git user
        git::git_cmd("git config")
            .unwrap()
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        git::git_cmd("git config")
            .unwrap()
            .args(["config", "user.name", "Test"])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        // First commit (required before creating a branch)
        git::git_cmd("git commit")
            .unwrap()
            .args(["commit", "--allow-empty", "-m", "initial"])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        // Create a trunk branch (avoid dangling commits)
        git::git_cmd("git checkout")
            .unwrap()
            .args(["branch", "-M", "trunk"])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        tmp
    }

    async fn create_commit(repo: &Path, message: &str) {
        git::git_cmd("git commit")
            .unwrap()
            .arg("commit")
            .arg("--allow-empty")
            .arg("-m")
            .arg(message)
            .current_dir(repo)
            .remove_git_envs()
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
            .arg("commit")
            .arg("--allow-empty")
            .arg("-m")
            .arg(message)
            .env("GIT_AUTHOR_DATE", &date_str)
            .env("GIT_COMMITTER_DATE", &date_str)
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();
    }

    async fn create_lightweight_tag(repo: &Path, tag: &str) {
        git::git_cmd("git tag")
            .unwrap()
            .arg("tag")
            .arg(tag)
            .arg("--no-sign")
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();
    }

    async fn create_annotated_tag(repo: &Path, tag: &str, days_ago: u64) {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            - (days_ago * 86400);

        let date_str = format!("{timestamp} +0000");

        git::git_cmd("git tag")
            .unwrap()
            .arg("tag")
            .arg(tag)
            .arg("-m")
            .arg(tag)
            .env("GIT_AUTHOR_DATE", &date_str)
            .env("GIT_COMMITTER_DATE", &date_str)
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();
    }

    fn get_backdated_timestamp(days_ago: u64) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now - (days_ago * 86400)
    }

    #[tokio::test]
    async fn test_get_tag_timestamps() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "old", 5).await;
        create_lightweight_tag(repo, "v0.1.0").await;

        create_backdated_commit(repo, "new", 2).await;
        create_lightweight_tag(repo, "v0.2.0").await;
        create_annotated_tag(repo, "alias-v0.2.0", 0).await;

        let timestamps = get_tag_timestamps(repo).await.unwrap();
        assert_eq!(timestamps.len(), 3);
        assert_eq!(timestamps[0].0, "alias-v0.2.0");
        assert_eq!(timestamps[1].0, "v0.2.0");
        assert_eq!(timestamps[2].0, "v0.1.0");
    }

    #[tokio::test]
    async fn test_resolve_bleeding_edge_prefers_exact_tag() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_commit(repo, "tagged").await;
        create_lightweight_tag(repo, "v1.2.3").await;

        git::git_cmd("git fetch")
            .unwrap()
            .args(["fetch", ".", "HEAD"])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        let rev = resolve_bleeding_edge(repo).await.unwrap();
        assert_eq!(rev, Some("v1.2.3".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_bleeding_edge_falls_back_to_rev_parse() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_commit(repo, "untagged").await;

        git::git_cmd("git fetch")
            .unwrap()
            .args(["fetch", ".", "HEAD"])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        let rev = resolve_bleeding_edge(repo).await.unwrap();

        let head = git::git_cmd("git rev-parse")
            .unwrap()
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap()
            .stdout;
        let head = String::from_utf8_lossy(&head).trim().to_string();

        assert_eq!(rev, Some(head));
    }

    #[tokio::test]
    async fn test_resolve_revision_uses_cooldown_bucket() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "candidate", 5).await;
        create_lightweight_tag(repo, "v2.0.0-rc1").await;
        create_lightweight_tag(repo, "totally-different").await;

        create_backdated_commit(repo, "latest", 1).await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let rev = resolve_revision(repo, "v2.0.0", false, 3).await.unwrap();

        assert_eq!(rev, Some("v2.0.0-rc1".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_revision_returns_none_when_all_tags_too_new() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "recent-1", 2).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "recent-2", 1).await;
        create_lightweight_tag(repo, "v1.1.0").await;

        let rev = resolve_revision(repo, "v1.1.0", false, 5).await.unwrap();

        assert_eq!(rev, None);
    }

    #[tokio::test]
    async fn test_resolve_revision_picks_oldest_eligible_bucket() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "oldest", 10).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "mid", 4).await;
        create_lightweight_tag(repo, "v1.1.0").await;

        create_backdated_commit(repo, "newest", 1).await;
        create_lightweight_tag(repo, "v1.2.0").await;

        let rev = resolve_revision(repo, "v1.2.0", false, 5).await.unwrap();

        assert_eq!(rev, Some("v1.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_revision_prefers_version_like_tags() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "eligible", 2).await;
        create_lightweight_tag(repo, "moving-tag").await;
        create_lightweight_tag(repo, "v1.0.0").await;

        // Even though the current rev matches the moving tag exactly, the dotted tag
        // should be preferred.
        let rev = resolve_revision(repo, "moving-tag", false, 1)
            .await
            .unwrap();

        assert_eq!(rev, Some("v1.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_revision_picks_closest_version_string() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "eligible", 3).await;
        create_lightweight_tag(repo, "v1.2.0").await;
        create_lightweight_tag(repo, "foo-1.2.0").await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let rev = resolve_revision(repo, "v1.2.3", false, 1).await.unwrap();

        assert_eq!(rev, Some("v1.2.0".to_string()));
    }
}
