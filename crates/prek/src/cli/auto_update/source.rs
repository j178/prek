use std::path::Path;

use anyhow::{Context, Result};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::{debug, trace, warn};

use crate::cli::auto_update::config::read_frozen_refs;
use crate::cli::auto_update::repository::{
    checkout_and_validate_manifest, get_tags_pointing_at_revision, is_commit_present,
    list_tag_metadata, resolve_revision_to_commit, select_best_tag, select_update_revision,
    setup_and_fetch_repo,
};
use crate::cli::auto_update::{
    CommitPresence, FrozenMismatch, FrozenMismatchAction, FrozenMismatchReason, RepoSource,
    RepoTarget, RepoUpdate, RepoUsage, ResolvedRepoUpdate, Revision, TagTimestamp,
};
use crate::config::{Repo, looks_like_sha};
use crate::fs::Simplified;
use crate::workspace::Workspace;

/// Collects the configured remote repos grouped by fetch source, revision, and hook set.
pub(super) fn collect_repo_sources(workspace: &Workspace) -> Result<Vec<RepoSource<'_>>> {
    let mut repo_sources: FxHashMap<&str, FxHashMap<(&str, Vec<&str>), RepoTarget<'_>>> =
        FxHashMap::default();

    for project in workspace.projects() {
        let remote_count = project
            .config()
            .repos
            .iter()
            .filter(|repo| matches!(repo, Repo::Remote(_)))
            .count();

        let frozen_refs = read_frozen_refs(project.config_file()).with_context(|| {
            format!(
                "Failed to read frozen references from `{}`",
                project.config_file().user_display()
            )
        })?;

        if frozen_refs.len() != remote_count {
            anyhow::bail!(
                "Found {} remote repos in `{}` but {} `rev:` entries while checking frozen refs",
                remote_count,
                project.config_file().user_display(),
                frozen_refs.len()
            );
        }

        let mut remote_index = 0;
        for repo in &project.config().repos {
            let Repo::Remote(remote_repo) = repo else {
                continue;
            };

            let mut required_hook_ids = remote_repo
                .hooks
                .iter()
                .map(|hook| hook.id.as_str())
                .collect::<Vec<_>>();
            required_hook_ids.sort_unstable();
            required_hook_ids.dedup();

            let target = repo_sources
                .entry(remote_repo.repo.as_str())
                .or_default()
                .entry((remote_repo.rev.as_str(), required_hook_ids.clone()))
                .or_insert_with(|| RepoTarget {
                    repo: remote_repo.repo.as_str(),
                    current_rev: remote_repo.rev.as_str(),
                    required_hook_ids,
                    usages: Vec::new(),
                });
            target.usages.push(RepoUsage {
                project,
                remote_count,
                remote_index,
                rev_line_number: frozen_refs[remote_index].line_number,
                current_frozen: frozen_refs[remote_index].current_frozen.clone(),
                current_frozen_site: frozen_refs[remote_index].site.clone(),
            });
            remote_index += 1;
        }
    }

    Ok(repo_sources
        .into_iter()
        .map(|(repo, targets)| {
            let mut targets = targets.into_values().collect::<Vec<_>>();
            targets.sort_by(|a, b| {
                a.current_rev
                    .cmp(b.current_rev)
                    .then_with(|| a.required_hook_ids.cmp(&b.required_hook_ids))
            });
            RepoSource { repo, targets }
        })
        .collect())
}

/// Collects stale `# frozen:` comments for one configured `repo + rev + hook set` target.
async fn collect_frozen_mismatches<'a>(
    repo_path: &Path,
    target: &'a RepoTarget<'a>,
    tag_timestamps: &[TagTimestamp],
) -> Result<Vec<FrozenMismatch<'a>>> {
    if !(target.current_rev.len() == 40 && looks_like_sha(target.current_rev)) {
        return Ok(Vec::new());
    }

    let frozen_refs_to_check = target
        .usages
        .iter()
        .filter_map(|usage| usage.current_frozen.as_deref())
        .collect::<FxHashSet<_>>();
    if frozen_refs_to_check.is_empty() {
        return Ok(Vec::new());
    }

    let current_rev_presence = is_commit_present(repo_path, target.current_rev).await?;
    let rev_tags = get_tags_pointing_at_revision(tag_timestamps, target.current_rev);
    let mut resolved_frozen_refs = FxHashMap::default();
    for frozen_ref in frozen_refs_to_check {
        let resolved = resolve_revision_to_commit(repo_path, frozen_ref).await.ok();
        resolved_frozen_refs.insert(frozen_ref, resolved);
    }

    Ok(target
        .usages
        .iter()
        .filter_map(|usage| {
            let current_frozen = usage.current_frozen.as_deref()?;
            let frozen_commit = resolved_frozen_refs.get(current_frozen).cloned().flatten();

            let reason = match frozen_commit.as_deref() {
                Some(frozen_commit) if frozen_commit.eq_ignore_ascii_case(target.current_rev) => {
                    return None;
                }
                Some(_) => FrozenMismatchReason::ResolvesToDifferentCommit,
                None => FrozenMismatchReason::Unresolvable,
            };
            let action = match select_best_tag(&rev_tags, current_frozen, true) {
                Some(replacement) => FrozenMismatchAction::ReplaceWith(replacement.to_string()),
                None => match current_rev_presence {
                    CommitPresence::Present => FrozenMismatchAction::Remove,
                    CommitPresence::Absent | CommitPresence::Unknown => {
                        FrozenMismatchAction::NoReplacement
                    }
                },
            };
            Some(FrozenMismatch {
                project: usage.project,
                remote_size: usage.remote_count,
                remote_index: usage.remote_index,
                rev_line_number: usage.rev_line_number,
                current_frozen: current_frozen.to_string(),
                frozen_site: usage.current_frozen_site.clone(),
                reason,
                current_rev_presence,
                action,
            })
        })
        .collect())
}

/// Fetches a remote repository once, then evaluates all configured revisions that use it.
pub(super) async fn evaluate_repo_source<'a>(
    repo_source: &'a RepoSource<'a>,
    bleeding_edge: bool,
    freeze: bool,
    cooldown_days: u8,
) -> Result<Vec<RepoUpdate<'a>>> {
    let tmp_dir = tempfile::tempdir()?;
    let repo_path = tmp_dir.path();

    let result = async {
        trace!(
            "Cloning repository `{}` to `{}`",
            repo_source.repo,
            repo_path.display()
        );
        setup_and_fetch_repo(repo_source.repo, repo_path).await?;
        let metadata = list_tag_metadata(repo_path).await?;

        anyhow::Ok(metadata)
    }
    .await;

    let tag_timestamps = match result {
        Ok(metadata) => metadata,
        Err(e) => {
            let error = format!("{e:#}");
            return Ok(repo_source
                .targets
                .iter()
                .map(|target| RepoUpdate {
                    target,
                    result: Err(anyhow::anyhow!(error.clone())),
                })
                .collect());
        }
    };

    let mut updates = Vec::with_capacity(repo_source.targets.len());
    for target in &repo_source.targets {
        let result = evaluate_repo_target(
            repo_path,
            target,
            bleeding_edge,
            freeze,
            cooldown_days,
            &tag_timestamps,
        )
        .await;

        updates.push(RepoUpdate { target, result });
    }

    Ok(updates)
}

/// Resolves one configured `repo + rev + hook set` entry within an already-fetched remote repository.
async fn evaluate_repo_target<'a>(
    repo_path: &Path,
    target: &'a RepoTarget<'a>,
    bleeding_edge: bool,
    freeze: bool,
    cooldown_days: u8,
    tag_timestamps: &[TagTimestamp],
) -> Result<ResolvedRepoUpdate<'a>> {
    let frozen_mismatches = match collect_frozen_mismatches(repo_path, target, tag_timestamps).await
    {
        Ok(mismatches) => mismatches,
        Err(e) => {
            warn!(
                "Failed to collect frozen comment context for repo `{}`: {e}",
                target.repo
            );
            Vec::new()
        }
    };

    let rev = select_update_revision(
        repo_path,
        target.current_rev,
        bleeding_edge,
        cooldown_days,
        tag_timestamps,
    )
    .await?;

    let Some(rev) = rev else {
        debug!("No suitable revision found for repo `{}`", target.repo);
        return Ok(ResolvedRepoUpdate {
            revision: Revision {
                rev: target.current_rev.to_string(),
                frozen: None,
            },
            frozen_mismatches,
        });
    };

    let (rev, frozen) = if freeze {
        let exact = resolve_revision_to_commit(repo_path, &rev).await?;
        if rev.eq_ignore_ascii_case(&exact) {
            (rev, None)
        } else {
            debug!("Freezing revision `{rev}` to `{exact}`");
            (exact, Some(rev))
        }
    } else {
        (rev, None)
    };

    checkout_and_validate_manifest(repo_path, &rev, &target.required_hook_ids).await?;

    Ok(ResolvedRepoUpdate {
        revision: Revision { rev, frozen },
        frozen_mismatches,
    })
}
