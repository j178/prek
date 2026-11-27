use std::borrow::Cow;
use std::cmp::Ordering;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Result;
use aws_lc_rs::digest::{SHA256, digest};
use itertools::Itertools;
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_consts::env_vars::EnvVars;
use rustc_hash::FxHashSet;
use semver::Version;
use tracing::{debug, trace};

use crate::cli::update::{CommitPresence, RevisionSelection, SkippedDowngrade, TagTimestamp};
use crate::fs::LockedFile;
use crate::git::GitCommandExt;
use crate::process::Cmd;
use crate::store::{REPO_MARKER, Store};
use crate::{config, git};

/// Each check gets a clean index and worktree. The object database and upstream
/// refs survive until cache GC. A source lock keeps those refs stable throughout
/// selection and manifest validation, even when two update processes overlap.
pub(super) struct UpdateRepository<'a> {
    worktree: PathBuf,
    scratch: &'a Path,
    source: &'a str,
}

impl<'a> UpdateRepository<'a> {
    pub(super) fn new(scratch: &'a Path, source: &'a str) -> Self {
        Self {
            worktree: scratch.join("worktree"),
            scratch,
            source,
        }
    }

    fn command(&self) -> Result<Cmd> {
        let mut cmd = git::git_cmd()?;
        // Manifest checkout can also fetch missing blobs. Supply the URL to
        // every command so credentials stay out of the cached Git config.
        cmd.hidden_args(["-c", &format!("remote.origin.url={}", self.source)])
            .current_dir(&self.worktree)
            .sanitize_git_repo_env()
            .env("GIT_INDEX_FILE", self.scratch.join("index"));
        Ok(cmd)
    }

    /// Refresh the remote refs before using any cached objects. The caller
    /// holds the lock until it has validated all targets against these refs.
    pub(super) async fn refresh(
        &self,
        store: &Store,
        installed_revs: impl Iterator<Item = &'a str>,
    ) -> Result<(Vec<TagTimestamp>, LockedFile)> {
        let key = digest(&SHA256, self.source.as_bytes());
        let root = store.updates_dir().join(hex::encode(key.as_ref()));
        fs_err::create_dir_all(&root)?;
        let lock = LockedFile::acquire(root.join("lock"), "repository update").await?;
        let git_dir = root.join("git");
        let incomplete = root.join("incomplete");

        // A killed fetch can leave Git lockfiles behind. Rebuild this source on
        // the next check if its last refresh never finished.
        if incomplete.try_exists()? && git_dir.try_exists()? {
            fs_err::remove_dir_all(&git_dir)?;
        }
        let cold = !git_dir.try_exists()?;
        fs_err::write(&incomplete, [])?;

        git::git_cmd()?
            .hidden_args(["-c", "init.defaultObjectFormat="])
            .args(["init", "--quiet", "--template=", "--separate-git-dir"])
            .arg(&git_dir)
            .arg(&self.worktree)
            .sanitize_git_repo_env()
            .output()
            .await?;

        let refspecs = [
            "--quiet",
            "--prune",
            "--no-tags",
            "--filter=blob:none",
            "origin",
            "+HEAD:refs/prek/upstream",
            "+refs/tags/*:refs/tags/*",
        ];
        let fetch = self
            .command()?
            .args(["fetch", "--no-write-fetch-head"])
            .args(refspecs)
            .stderr(Stdio::null())
            .run()
            .await;

        match fetch {
            // Git before 2.29 cannot suppress FETCH_HEAD, which can contain
            // the remote URL. Keep its fetch in this check's temporary repo.
            Err(crate::process::Error::Status { error, .. })
                if error.status.code() == Some(129) =>
            {
                fs_err::remove_file(self.worktree.join(".git"))?;
                git::init_repo(self.source, &self.worktree).await?;
                self.command()?
                    .arg("fetch")
                    .args(refspecs)
                    .stderr(Stdio::null())
                    .run()
                    .await?;
            }
            result => result?,
        }

        if cold && let Err(err) = self.seed_manifest(store, installed_revs).await {
            trace!("Could not reuse installed manifest: {err}");
        }

        let tags = list_tag_metadata(self).await?;
        fs_err::remove_file(incomplete)?;
        Ok((tags, lock))
    }

    /// The installed and candidate revisions often share their manifest blob.
    /// Import just that Git object, so a shallow checkout or a dirty worktree
    /// cannot change which refs or manifest content the update sees.
    async fn seed_manifest(
        &self,
        store: &Store,
        installed_revs: impl Iterator<Item = &'a str>,
    ) -> Result<()> {
        let Some(checkout) = installed_revs
            .map(|rev| store.repo_path(self.source, rev))
            .find(|path| path.join(REPO_MARKER).is_file())
        else {
            return Ok(());
        };

        let manifest = tempfile::NamedTempFile::new_in(self.scratch)?;
        git::git_cmd()?
            .current_dir(checkout)
            .sanitize_git_repo_env()
            .args(["--no-lazy-fetch", "cat-file", "blob"])
            .arg(format!("HEAD:{PRE_COMMIT_HOOKS_YAML}"))
            .stdout(manifest.reopen()?)
            .stderr(Stdio::null())
            .run()
            .await?;

        self.command()?
            .args(["hash-object", "-w", "--no-filters", "--"])
            .arg(manifest.path())
            .output()
            .await?;
        Ok(())
    }
}

/// Resolves any revision-like string to the underlying commit SHA.
pub(super) async fn resolve_revision_to_commit(
    repo: &UpdateRepository<'_>,
    rev: &str,
) -> Result<String> {
    let output = repo
        .command()?
        .arg("rev-parse")
        .arg(format!("{rev}^{{}}"))
        .check(true)
        .output()
        .await?;

    Ok(str::from_utf8(&output.stdout)?.trim().to_string())
}

/// Old objects can survive a retag or a force push. Only commits reachable
/// from the current upstream HEAD or tags count as present. Disable lazy
/// fetching so looking up an unknown SHA never requests it from the remote.
pub(super) async fn is_commit_present(
    repo: &UpdateRepository<'_>,
    commit: &str,
) -> Result<CommitPresence> {
    static GIT_SUPPORTS_NO_LAZY_FETCH: OnceLock<bool> = OnceLock::new();

    if matches!(GIT_SUPPORTS_NO_LAZY_FETCH.get(), Some(false)) {
        return Ok(CommitPresence::Unknown);
    }

    let output = repo
        .command()?
        .arg("--no-lazy-fetch")
        .args([
            "for-each-ref",
            "--count=1",
            "--format=%(refname)",
            "--contains",
        ])
        .arg(commit)
        .args(["refs/prek/upstream", "refs/tags"])
        .env(EnvVars::LC_ALL, "C")
        .check(false)
        .output()
        .await?;

    if output.status.success() {
        let _ = GIT_SUPPORTS_NO_LAZY_FETCH.set(true);
        return Ok(if output.stdout.is_empty() {
            CommitPresence::Absent
        } else {
            CommitPresence::Present
        });
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
pub(super) async fn resolve_bleeding_edge(repo: &UpdateRepository<'_>) -> Result<Option<String>> {
    let output = repo
        .command()?
        .arg("describe")
        .arg("refs/prek/upstream")
        .arg("--tags")
        .arg("--exact-match")
        .check(false)
        .output()
        .await?;
    let rev = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        debug!("No matching tag for the upstream HEAD, using rev-parse instead");
        let output = repo
            .command()?
            .arg("rev-parse")
            .arg("refs/prek/upstream")
            .check(true)
            .output()
            .await?;
        str::from_utf8(&output.stdout)?.trim().to_string()
    };

    debug!("Resolved the upstream HEAD to `{rev}`");
    Ok(Some(rev))
}

/// Lists fetched tag metadata sorted from newest to oldest timestamp.
///
/// Within groups of tags sharing the same timestamp, semver-parseable tags
/// are sorted highest version first; non-semver tags sort after them.
async fn list_tag_metadata(repo: &UpdateRepository<'_>) -> Result<Vec<TagTimestamp>> {
    let output = repo
        .command()?
        .arg("for-each-ref")
        .arg("--sort=-creatordate")
        .arg("--format=%(refname:lstrip=2)\t%(creatordate:unix)\t%(objectname)\t%(*objectname)")
        .arg("refs/tags")
        .check(true)
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
    repo: &UpdateRepository<'_>,
    current_rev: &str,
    tag_timestamps: &'a [TagTimestamp],
) -> Option<&'a TagTimestamp> {
    if let Some(tag) = tag_timestamps.iter().find(|tag| tag.tag == current_rev) {
        return Some(tag);
    }

    let current_commit = if current_rev.len() == 40 && config::looks_like_sha(current_rev) {
        Cow::Borrowed(current_rev)
    } else {
        Cow::Owned(resolve_revision_to_commit(repo, current_rev).await.ok()?)
    };

    tag_timestamps
        .iter()
        .filter(|tag| tag.commit.eq_ignore_ascii_case(current_commit.as_ref()))
        .min_by(|tag_a, tag_b| compare_tag_metadata(tag_a, tag_b))
}

/// Selects the revision action that `prek update` should take for one fetched repo target.
///
/// In normal mode this chooses the newest tag that satisfies the cooldown window.
/// If that tag sorts older than the currently pinned tag, the current revision is kept.
/// In bleeding-edge mode it resolves the upstream HEAD instead.
pub(super) async fn select_update_revision(
    repo: &UpdateRepository<'_>,
    current_rev: &str,
    bleeding_edge: bool,
    cooldown_days: u8,
    tag_timestamps: &[TagTimestamp],
    update_tag_timestamps: &[TagTimestamp],
) -> Result<RevisionSelection> {
    if bleeding_edge {
        return Ok(match resolve_bleeding_edge(repo).await? {
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
        && let Some(current_tag) = current_tag_metadata(repo, current_rev, tag_timestamps).await
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
    let candidates = if has_version_like {
        tags.iter()
            .filter(|tag| tag.contains('.'))
            .copied()
            .collect::<Vec<_>>()
    } else if allow_non_version_like {
        tags.to_vec()
    } else {
        return None;
    };

    candidates
        .into_iter()
        .map(|tag| (levenshtein_distance(tag, current_ref), tag))
        .min_by(|(distance_a, tag_a), (distance_b, tag_b)| {
            distance_a
                .cmp(distance_b)
                .then_with(|| compare_tag_versions_desc(tag_a, tag_b))
                .then_with(|| tag_a.cmp(tag_b))
        })
        .map(|(_, tag)| tag)
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    if a == b {
        return 0;
    }

    let a_len = a.chars().count();
    let b_len = b.chars().count();

    if a_len == 0 {
        return b_len;
    }

    if b_len == 0 {
        return a_len;
    }

    let mut row = (0..=a_len).collect::<Vec<_>>();

    for (b_index, b_char) in b.chars().enumerate() {
        let mut previous = row[0];
        row[0] = b_index + 1;

        for (a_index, a_char) in a.chars().enumerate() {
            let deletion = row[a_index + 1] + 1;
            let insertion = row[a_index] + 1;
            let substitution = previous + usize::from(a_char != b_char);

            previous = row[a_index + 1];
            row[a_index + 1] = deletion.min(insertion).min(substitution);
        }
    }

    row[a_len]
}

/// Checks out the candidate manifest and verifies all configured hook ids still exist.
pub(super) async fn checkout_and_validate_manifest(
    repo: &UpdateRepository<'_>,
    rev: &str,
    required_hook_ids: &[&str],
) -> Result<()> {
    if cfg!(windows) {
        repo.command()?
            .arg("show")
            .arg(format!("{rev}:{PRE_COMMIT_HOOKS_YAML}"))
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
    }

    repo.command()?
        .arg("checkout")
        .arg("--quiet")
        .arg(rev)
        .arg("--")
        .arg(PRE_COMMIT_HOOKS_YAML)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    let manifest = config::read_manifest(&repo.worktree.join(PRE_COMMIT_HOOKS_YAML))?;
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
        UpdateRepository, levenshtein_distance, list_tag_metadata, no_lazy_fetch_unsupported,
        resolve_bleeding_edge, select_best_tag, select_update_revision,
    };
    use crate::cli::update::{RevisionSelection, SkippedDowngrade};
    use crate::git;
    use crate::git::GitCommandExt;
    use crate::process::Cmd;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    async fn setup_test_repo() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        let repo = &tmp.path().join("worktree");

        git::git_cmd()
            .unwrap()
            .arg("init")
            .arg(repo)
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();

        git::git_cmd()
            .unwrap()
            .args(["config", "user.email", "test@test.com"])
            .current_dir(repo)
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();

        git::git_cmd()
            .unwrap()
            .args(["config", "user.name", "Test"])
            .current_dir(repo)
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();

        git::git_cmd()
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
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();

        git::git_cmd()
            .unwrap()
            .args(["branch", "-M", "trunk"])
            .current_dir(repo)
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();

        tmp
    }

    fn git_cmd(dir: impl AsRef<Path>) -> Cmd {
        let mut cmd = git::git_cmd().unwrap();
        cmd.current_dir(dir)
            .args(["-c", "commit.gpgsign=false"])
            .args(["-c", "tag.gpgsign=false"]);
        cmd
    }

    #[test]
    fn test_levenshtein_distance() {
        assert_eq!(levenshtein_distance("", ""), 0);
        assert_eq!(levenshtein_distance("", "v1.0.0"), 6);
        assert_eq!(levenshtein_distance("v1.0.0", "v1.1.0"), 1);
        assert_eq!(levenshtein_distance("v1.0.0", "foo-v1.1.0"), 5);
        assert_eq!(levenshtein_distance("v1.0.0", "v1.0.0"), 0);
        assert_eq!(levenshtein_distance("mañana", "manana"), 1);
    }

    #[test]
    fn test_select_best_tag_prefers_closest_tag() {
        let tags = ["v1.1.0", "foo-v1.1.0", "v1"];

        assert_eq!(select_best_tag(&tags, "v1.0.0", false), Some("v1.1.0"));
    }

    async fn create_commit(repo: &Path, message: &str) {
        git_cmd(repo)
            .args(["commit", "--allow-empty", "-m", message])
            .sanitize_git_repo_env()
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

        git_cmd(repo)
            .args(["commit", "--allow-empty", "-m", message])
            .env("GIT_AUTHOR_DATE", &date_str)
            .env("GIT_COMMITTER_DATE", &date_str)
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();
    }

    async fn create_lightweight_tag(repo: &Path, tag: &str) {
        git_cmd(repo)
            .arg("tag")
            .arg(tag)
            .sanitize_git_repo_env()
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

        git_cmd(repo)
            .arg("tag")
            .arg(tag)
            .arg("-m")
            .arg(tag)
            .env("GIT_AUTHOR_DATE", &date_str)
            .env("GIT_COMMITTER_DATE", &date_str)
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn test_list_tag_metadata() {
        let tmp = setup_test_repo().await;
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "old", 5).await;
        create_lightweight_tag(repo, "v0.1.0").await;

        create_backdated_commit(repo, "new", 2).await;
        create_lightweight_tag(repo, "v0.2.0").await;
        create_annotated_tag(repo, "alias-v0.2.0", 0).await;

        let timestamps = list_tag_metadata(&UpdateRepository::new(tmp.path(), "."))
            .await
            .unwrap();
        assert_eq!(timestamps.len(), 3);
        assert_eq!(timestamps[0].tag, "alias-v0.2.0");
        assert_eq!(timestamps[1].tag, "v0.2.0");
        assert_eq!(timestamps[2].tag, "v0.1.0");
        assert_eq!(timestamps[0].commit, timestamps[1].commit);
    }

    #[tokio::test]
    async fn test_resolve_bleeding_edge_prefers_exact_tag() {
        let tmp = setup_test_repo().await;
        let repo = &tmp.path().join("worktree");

        create_commit(repo, "tagged").await;
        create_lightweight_tag(repo, "v1.2.3").await;

        git::git_cmd()
            .unwrap()
            .args(["fetch", ".", "HEAD:refs/prek/upstream"])
            .current_dir(repo)
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();

        let rev = resolve_bleeding_edge(&UpdateRepository::new(tmp.path(), "."))
            .await
            .unwrap();
        assert_eq!(rev, Some("v1.2.3".to_string()));
    }

    #[tokio::test]
    async fn test_resolve_bleeding_edge_falls_back_to_rev_parse() {
        let tmp = setup_test_repo().await;
        let repo = &tmp.path().join("worktree");

        create_commit(repo, "untagged").await;

        git::git_cmd()
            .unwrap()
            .args(["fetch", ".", "HEAD:refs/prek/upstream"])
            .current_dir(repo)
            .sanitize_git_repo_env()
            .output()
            .await
            .unwrap();

        let rev = resolve_bleeding_edge(&UpdateRepository::new(tmp.path(), "."))
            .await
            .unwrap();

        let head = git::git_cmd()
            .unwrap()
            .args(["rev-parse", "HEAD"])
            .current_dir(repo)
            .sanitize_git_repo_env()
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
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "current", 10).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "candidate", 5).await;
        create_lightweight_tag(repo, "v2.0.0-rc1").await;
        create_lightweight_tag(repo, "totally-different").await;

        create_backdated_commit(repo, "latest", 1).await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let repo = &UpdateRepository::new(tmp.path(), ".");
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
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "candidate", 5).await;
        create_lightweight_tag(repo, "v2.0.0-rc1").await;

        create_backdated_commit(repo, "current", 1).await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let repo = &UpdateRepository::new(tmp.path(), ".");
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
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "recent-1", 2).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "recent-2", 1).await;
        create_lightweight_tag(repo, "v1.1.0").await;

        let repo = &UpdateRepository::new(tmp.path(), ".");
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
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "oldest", 10).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "mid", 4).await;
        create_lightweight_tag(repo, "v1.1.0").await;

        create_backdated_commit(repo, "newest", 1).await;
        create_lightweight_tag(repo, "v1.2.0").await;

        let repo = &UpdateRepository::new(tmp.path(), ".");
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
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "eligible", 2).await;
        create_lightweight_tag(repo, "moving-tag").await;
        create_lightweight_tag(repo, "v1.0.0").await;

        let repo = &UpdateRepository::new(tmp.path(), ".");
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
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "eligible", 3).await;
        create_lightweight_tag(repo, "v1.2.0").await;
        create_lightweight_tag(repo, "foo-1.2.0").await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let repo = &UpdateRepository::new(tmp.path(), ".");
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
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "release", 5).await;
        create_lightweight_tag(repo, "v1.0.0").await;
        create_lightweight_tag(repo, "v1.0.3").await;
        create_lightweight_tag(repo, "v1.0.5").await;
        create_lightweight_tag(repo, "v1.0.2").await;

        let timestamps = list_tag_metadata(&UpdateRepository::new(tmp.path(), "."))
            .await
            .unwrap();

        let tags: Vec<&str> = timestamps.iter().map(|tag| tag.tag.as_str()).collect();
        assert_eq!(tags, vec!["v1.0.5", "v1.0.3", "v1.0.2", "v1.0.0"]);
    }

    #[tokio::test]
    async fn test_list_tag_metadata_deterministic_order_for_equal_timestamp_non_semver() {
        let tmp = setup_test_repo().await;
        let repo = &tmp.path().join("worktree");

        create_backdated_commit(repo, "release", 5).await;
        create_lightweight_tag(repo, "beta").await;
        create_lightweight_tag(repo, "alpha").await;
        create_lightweight_tag(repo, "gamma").await;

        let timestamps = list_tag_metadata(&UpdateRepository::new(tmp.path(), "."))
            .await
            .unwrap();
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
