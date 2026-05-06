use std::borrow::Cow;
use std::cmp::Ordering;
use std::path::Path;
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use itertools::Itertools;
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_consts::env_vars::EnvVars;
use rustc_hash::FxHashSet;
use semver::Version;
use tracing::{debug, trace};

use crate::cli::auto_update::{CommitPresence, RevisionSelection, SkippedDowngrade, TagTimestamp};
use crate::{config, git};

/// Initializes a temporary git repo and fetches the remote HEAD plus tags.
pub(super) async fn setup_and_fetch_repo(repo_url: &str, repo_path: &Path) -> Result<()> {
    git::init_repo(repo_url, repo_path).await?;
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

/// Resolves any revision-like string to the underlying commit SHA.
pub(super) async fn resolve_revision_to_commit(repo_path: &Path, rev: &str) -> Result<String> {
    let output = git::git_cmd("git rev-parse")?
        .arg("rev-parse")
        .arg(format!("{rev}^{{}}"))
        .check(true)
        .current_dir(repo_path)
        .remove_git_envs()
        .output()
        .await?;

    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Returns whether a pinned commit SHA is already present in the refs fetched for `auto-update`.
///
/// `auto-update` fetches only `origin/HEAD` and tags, using `--filter=blob:none`. That filter
/// still downloads commits and trees reachable from those refs, but omits blobs. We intentionally
/// use `git --no-lazy-fetch cat-file -e` here instead of `rev-parse`: in a partial clone,
/// `rev-parse` may lazily fetch a missing commit from the promisor remote on demand. On GitHub,
/// that can make a fork-only "impostor commit" appear to belong to the parent repository.
///
/// `auto-update` only selects updates from tags, or from `HEAD` in `--bleeding-edge` mode. It
/// does not normally update to arbitrary branches, so we currently fetch only those refs here.
///
/// So this helper answers a narrower question than "is this SHA valid anywhere on the remote?".
/// It only checks whether the commit is already available from the refs we fetched for update
/// selection. That means branch-only commits outside `HEAD` and tags are treated as absent for
/// now. If that leads to false positives in practice, we can revisit this and fetch branches too.
///
/// On older Git versions that do not support `--no-lazy-fetch`, we skip this check entirely and
/// return `CommitPresence::Unknown` so the caller can avoid presenting inaccurate presence details.
pub(super) async fn is_commit_present(repo_path: &Path, commit: &str) -> Result<CommitPresence> {
    static GIT_SUPPORTS_NO_LAZY_FETCH: OnceLock<bool> = OnceLock::new();

    if matches!(GIT_SUPPORTS_NO_LAZY_FETCH.get(), Some(false)) {
        return Ok(CommitPresence::Unknown);
    }

    let output = git::git_cmd("git cat-file")?
        .arg("--no-lazy-fetch")
        .arg("cat-file")
        .arg("-e")
        .arg(format!("{commit}^{{commit}}"))
        .env(EnvVars::LC_ALL, "C")
        .check(false)
        .current_dir(repo_path)
        .remove_git_envs()
        .stdout(Stdio::null())
        .output()
        .await?;

    if output.status.success() {
        let _ = GIT_SUPPORTS_NO_LAZY_FETCH.set(true);
        return Ok(CommitPresence::Present);
    }

    if no_lazy_fetch_unsupported(&output.stderr) {
        let _ = GIT_SUPPORTS_NO_LAZY_FETCH.set(false);
        return Ok(CommitPresence::Unknown);
    }

    let _ = GIT_SUPPORTS_NO_LAZY_FETCH.set(true);
    Ok(CommitPresence::Absent)
}

pub(super) fn no_lazy_fetch_unsupported(stderr: &[u8]) -> bool {
    let stderr = String::from_utf8_lossy(stderr);
    stderr.contains("--no-lazy-fetch") && stderr.contains("unknown option")
}

pub(super) fn get_tags_pointing_at_revision<'a>(
    tag_timestamps: &'a [TagTimestamp],
    rev: &str,
) -> Vec<&'a str> {
    tag_timestamps
        .iter()
        .filter(|tag_timestamp| tag_timestamp.commit.eq_ignore_ascii_case(rev))
        .map(|tag_timestamp| tag_timestamp.tag.as_str())
        .collect()
}

/// Resolves the default branch tip to an exact tag when possible, otherwise to a commit SHA.
pub(super) async fn resolve_bleeding_edge(repo_path: &Path) -> Result<Option<String>> {
    let output = git::git_cmd("git describe")?
        .arg("describe")
        .arg("FETCH_HEAD")
        .arg("--tags")
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

/// Lists fetched tag metadata sorted from newest to oldest timestamp.
///
/// Within groups of tags sharing the same timestamp, semver-parseable tags
/// are sorted highest version first; non-semver tags sort after them.
pub(super) async fn list_tag_metadata(repo: &Path) -> Result<Vec<TagTimestamp>> {
    let output = git::git_cmd("git for-each-ref")?
        .arg("for-each-ref")
        .arg("--sort=-creatordate")
        .arg("--format=%(refname:lstrip=2)\t%(creatordate:unix)\t%(objectname)\t%(*objectname)")
        .arg("refs/tags")
        .check(true)
        .current_dir(repo)
        .remove_git_envs()
        .output()
        .await?;

    let mut tags: Vec<TagTimestamp> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| {
            let mut parts = line.split('\t');
            let tag = parts.next()?.trim_ascii();
            let ts_str = parts.next()?.trim_ascii();
            let object = parts.next()?.trim_ascii();
            let peeled = parts.next().unwrap_or_default().trim_ascii();
            let ts: u64 = ts_str.parse().ok()?;
            let commit = if peeled.is_empty() { object } else { peeled };
            Some(TagTimestamp::new(tag.to_string(), ts, commit.to_string()))
        })
        .collect();

    tags.sort_by(compare_tag_metadata);

    Ok(tags)
}

fn compare_tag_metadata(tag_a: &TagTimestamp, tag_b: &TagTimestamp) -> Ordering {
    tag_b
        .timestamp
        .cmp(&tag_a.timestamp)
        .then_with(|| match (&tag_a.version, &tag_b.version) {
            (Some(a), Some(b)) => b.cmp(a).then_with(|| tag_a.tag.cmp(&tag_b.tag)),
            (Some(_), None) => Ordering::Less,
            (None, Some(_)) => Ordering::Greater,
            (None, None) => tag_a.tag.cmp(&tag_b.tag),
        })
}

async fn current_tag_metadata<'a>(
    repo_path: &Path,
    current_rev: &str,
    tag_timestamps: &'a [TagTimestamp],
) -> Option<&'a TagTimestamp> {
    if let Some(tag) = tag_timestamps.iter().find(|tag| tag.tag == current_rev) {
        return Some(tag);
    }

    let current_commit = if current_rev.len() == 40 && config::looks_like_sha(current_rev) {
        Cow::Borrowed(current_rev)
    } else {
        Cow::Owned(
            resolve_revision_to_commit(repo_path, current_rev)
                .await
                .ok()?,
        )
    };

    tag_timestamps
        .iter()
        .filter(|tag| tag.commit.eq_ignore_ascii_case(current_commit.as_ref()))
        .min_by(|tag_a, tag_b| compare_tag_metadata(tag_a, tag_b))
}

/// Selects the revision action that `auto-update` should take for one fetched repo target.
///
/// In normal mode this chooses the newest tag that satisfies the cooldown window.
/// If that tag sorts older than the currently pinned tag, the current revision is kept.
/// In bleeding-edge mode it resolves `FETCH_HEAD` instead.
pub(super) async fn select_update_revision(
    repo_path: &Path,
    current_rev: &str,
    bleeding_edge: bool,
    cooldown_days: u8,
    tag_timestamps: &[TagTimestamp],
    update_tag_timestamps: &[TagTimestamp],
) -> Result<RevisionSelection> {
    if bleeding_edge {
        return Ok(match resolve_bleeding_edge(repo_path).await? {
            Some(rev) => RevisionSelection::Update(rev),
            None => RevisionSelection::Unchanged,
        });
    }

    let cutoff_secs = u64::from(cooldown_days) * 86400;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let cutoff = now.saturating_sub(cutoff_secs);

    let left =
        match update_tag_timestamps.binary_search_by(|tag| tag.timestamp.cmp(&cutoff).reverse()) {
            Ok(i) | Err(i) => i,
        };

    let Some(target_tag) = update_tag_timestamps.get(left) else {
        trace!("No tags meet cooldown cutoff {cutoff_secs}s");
        return Ok(RevisionSelection::Unchanged);
    };

    debug!(
        "Using tag `{}` cutoff timestamp {}",
        target_tag.tag, target_tag.timestamp
    );

    if cooldown_days > 0
        && let Some(current_tag) =
            current_tag_metadata(repo_path, current_rev, tag_timestamps).await
        && !current_tag.commit.eq_ignore_ascii_case(&target_tag.commit)
        && compare_tag_metadata(current_tag, target_tag).is_lt()
    {
        debug!(
            "Skipping candidate tag `{}` because current tag `{}` sorts newer",
            target_tag.tag, current_tag.tag
        );
        return Ok(RevisionSelection::SkippedDowngrade(SkippedDowngrade {
            current: current_rev.to_string(),
            candidate: target_tag.tag.clone(),
            cooldown_days,
        }));
    }

    let tags = get_tags_pointing_at_revision(update_tag_timestamps, &target_tag.commit);
    let best = select_best_tag(&tags, current_rev, false)
        .unwrap_or(target_tag.tag.as_str())
        .to_string();
    debug!(
        "Using best candidate tag `{best}` for revision `{}`",
        target_tag.tag
    );

    Ok(RevisionSelection::Update(best))
}

/// Orders version-like tags from newest to oldest semantic version.
fn compare_tag_versions_desc(tag_a: &str, tag_b: &str) -> std::cmp::Ordering {
    let version_a = Version::parse(tag_a.strip_prefix('v').unwrap_or(tag_a));
    let version_b = Version::parse(tag_b.strip_prefix('v').unwrap_or(tag_b));

    match (version_a, version_b) {
        (Ok(a), Ok(b)) => b.cmp(&a),
        (Ok(_), Err(_)) => std::cmp::Ordering::Less,
        (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
        (Err(_), Err(_)) => std::cmp::Ordering::Equal,
    }
}

/// Multiple tags can exist on an SHA. Sometimes a moving tag is attached to a
/// version tag. Prefer tags that look like versions, then pick the one most
/// similar to the current reference.
pub(super) fn select_best_tag<'a>(
    tags: &[&'a str],
    current_ref: &str,
    allow_non_version_like: bool,
) -> Option<&'a str> {
    let has_version_like = tags.iter().any(|tag| tag.contains('.'));
    let mut candidates = if has_version_like {
        tags.iter()
            .filter(|tag| tag.contains('.'))
            .copied()
            .collect::<Vec<_>>()
    } else if allow_non_version_like {
        tags.to_vec()
    } else {
        return None;
    };

    candidates.sort_by(|tag_a, tag_b| {
        levenshtein::levenshtein(tag_a, current_ref)
            .cmp(&levenshtein::levenshtein(tag_b, current_ref))
            .then_with(|| compare_tag_versions_desc(tag_a, tag_b))
            .then_with(|| tag_a.cmp(tag_b))
    });

    candidates.into_iter().next()
}

/// Checks out the candidate manifest and verifies all configured hook ids still exist.
pub(super) async fn checkout_and_validate_manifest(
    repo_path: &Path,
    rev: &str,
    required_hook_ids: &[&str],
) -> Result<()> {
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
    let hooks_missing = required_hook_ids
        .iter()
        .filter(|hook_id| !new_hook_ids.contains(**hook_id))
        .collect::<Vec<_>>();
    if !hooks_missing.is_empty() {
        anyhow::bail!(
            "Cannot update to rev `{}`, hook{} {} missing: {}",
            rev,
            if hooks_missing.len() > 1 { "s" } else { "" },
            if hooks_missing.len() > 1 { "are" } else { "is" },
            hooks_missing.into_iter().join(", ")
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        list_tag_metadata, no_lazy_fetch_unsupported, resolve_bleeding_edge, select_update_revision,
    };
    use crate::cli::auto_update::{RevisionSelection, SkippedDowngrade};
    use crate::git;
    use crate::process::Cmd;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    async fn setup_test_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let repo = tmp.path();

        git::git_cmd("git init")
            .unwrap()
            .arg("init")
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

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

        git::git_cmd("git commit")
            .unwrap()
            .args([
                "-c",
                "commit.gpgsign=false",
                "commit",
                "--allow-empty",
                "-m",
                "initial",
            ])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        git::git_cmd("git branch")
            .unwrap()
            .args(["branch", "-M", "trunk"])
            .current_dir(repo)
            .remove_git_envs()
            .output()
            .await
            .unwrap();

        tmp
    }

    fn git_cmd(dir: impl AsRef<Path>, summary: &str) -> Cmd {
        let mut cmd = git::git_cmd(summary).unwrap();
        cmd.current_dir(dir)
            .args(["-c", "commit.gpgsign=false"])
            .args(["-c", "tag.gpgsign=false"]);
        cmd
    }

    async fn create_commit(repo: &Path, message: &str) {
        git_cmd(repo, "git commit")
            .args(["commit", "--allow-empty", "-m", message])
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

        git_cmd(repo, "git commit")
            .args(["commit", "--allow-empty", "-m", message])
            .env("GIT_AUTHOR_DATE", &date_str)
            .env("GIT_COMMITTER_DATE", &date_str)
            .remove_git_envs()
            .output()
            .await
            .unwrap();
    }

    async fn create_lightweight_tag(repo: &Path, tag: &str) {
        git_cmd(repo, "git tag")
            .arg("tag")
            .arg(tag)
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

        git_cmd(repo, "git tag")
            .arg("tag")
            .arg(tag)
            .arg("-m")
            .arg(tag)
            .env("GIT_AUTHOR_DATE", &date_str)
            .env("GIT_COMMITTER_DATE", &date_str)
            .remove_git_envs()
            .output()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_list_tag_metadata() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "old", 5).await;
        create_lightweight_tag(repo, "v0.1.0").await;

        create_backdated_commit(repo, "new", 2).await;
        create_lightweight_tag(repo, "v0.2.0").await;
        create_annotated_tag(repo, "alias-v0.2.0", 0).await;

        let timestamps = list_tag_metadata(repo).await.unwrap();
        assert_eq!(timestamps.len(), 3);
        assert_eq!(timestamps[0].tag, "alias-v0.2.0");
        assert_eq!(timestamps[1].tag, "v0.2.0");
        assert_eq!(timestamps[2].tag, "v0.1.0");
        assert_eq!(timestamps[0].commit, timestamps[1].commit);
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
    async fn test_select_update_revision_uses_cooldown_bucket() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "current", 10).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "candidate", 5).await;
        create_lightweight_tag(repo, "v2.0.0-rc1").await;
        create_lightweight_tag(repo, "totally-different").await;

        create_backdated_commit(repo, "latest", 1).await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev =
            select_update_revision(repo, "v1.0.0", false, 3, &tag_timestamps, &tag_timestamps)
                .await
                .unwrap();

        assert_eq!(rev, RevisionSelection::Update("v2.0.0-rc1".to_string()));
    }

    #[tokio::test]
    async fn test_select_update_revision_skips_cooldown_downgrade() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "candidate", 5).await;
        create_lightweight_tag(repo, "v2.0.0-rc1").await;

        create_backdated_commit(repo, "current", 1).await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev =
            select_update_revision(repo, "v2.0.0", false, 3, &tag_timestamps, &tag_timestamps)
                .await
                .unwrap();

        assert_eq!(
            rev,
            RevisionSelection::SkippedDowngrade(SkippedDowngrade {
                current: "v2.0.0".to_string(),
                candidate: "v2.0.0-rc1".to_string(),
                cooldown_days: 3
            })
        );
    }

    #[tokio::test]
    async fn test_select_update_revision_returns_none_when_all_tags_too_new() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "recent-1", 2).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "recent-2", 1).await;
        create_lightweight_tag(repo, "v1.1.0").await;

        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev =
            select_update_revision(repo, "v1.1.0", false, 5, &tag_timestamps, &tag_timestamps)
                .await
                .unwrap();

        assert_eq!(rev, RevisionSelection::Unchanged);
    }

    #[tokio::test]
    async fn test_select_update_revision_skips_older_eligible_bucket() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "oldest", 10).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "mid", 4).await;
        create_lightweight_tag(repo, "v1.1.0").await;

        create_backdated_commit(repo, "newest", 1).await;
        create_lightweight_tag(repo, "v1.2.0").await;

        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev =
            select_update_revision(repo, "v1.2.0", false, 5, &tag_timestamps, &tag_timestamps)
                .await
                .unwrap();

        assert_eq!(
            rev,
            RevisionSelection::SkippedDowngrade(SkippedDowngrade {
                current: "v1.2.0".to_string(),
                candidate: "v1.0.0".to_string(),
                cooldown_days: 5
            })
        );
    }

    #[tokio::test]
    async fn test_select_update_revision_prefers_version_like_tags() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "eligible", 2).await;
        create_lightweight_tag(repo, "moving-tag").await;
        create_lightweight_tag(repo, "v1.0.0").await;

        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev = select_update_revision(
            repo,
            "moving-tag",
            false,
            1,
            &tag_timestamps,
            &tag_timestamps,
        )
        .await
        .unwrap();

        assert_eq!(rev, RevisionSelection::Update("v1.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_select_update_revision_picks_closest_version_string() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "eligible", 3).await;
        create_lightweight_tag(repo, "v1.2.0").await;
        create_lightweight_tag(repo, "foo-1.2.0").await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev =
            select_update_revision(repo, "v1.2.3", false, 1, &tag_timestamps, &tag_timestamps)
                .await
                .unwrap();

        assert_eq!(rev, RevisionSelection::Update("v1.2.0".to_string()));
    }

    #[tokio::test]
    async fn test_list_tag_metadata_stable_order_for_equal_timestamps() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "release", 5).await;
        create_lightweight_tag(repo, "v1.0.0").await;
        create_lightweight_tag(repo, "v1.0.3").await;
        create_lightweight_tag(repo, "v1.0.5").await;
        create_lightweight_tag(repo, "v1.0.2").await;

        let timestamps = list_tag_metadata(repo).await.unwrap();

        let tags: Vec<&str> = timestamps.iter().map(|tag| tag.tag.as_str()).collect();
        assert_eq!(tags, vec!["v1.0.5", "v1.0.3", "v1.0.2", "v1.0.0"]);
    }

    #[tokio::test]
    async fn test_list_tag_metadata_deterministic_order_for_equal_timestamp_non_semver() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "release", 5).await;
        create_lightweight_tag(repo, "beta").await;
        create_lightweight_tag(repo, "alpha").await;
        create_lightweight_tag(repo, "gamma").await;

        let timestamps = list_tag_metadata(repo).await.unwrap();
        let tags: Vec<&str> = timestamps.iter().map(|tag| tag.tag.as_str()).collect();
        assert_eq!(tags, vec!["alpha", "beta", "gamma"]);
    }

    #[test]
    fn test_no_lazy_fetch_unsupported() {
        assert!(no_lazy_fetch_unsupported(
            b"unknown option: --no-lazy-fetch\n"
        ));
        assert!(!no_lazy_fetch_unsupported(
            b"fatal: Not a valid object name 1234567890abcdef1234567890abcdef12345678^{commit}\n"
        ));
    }
}
