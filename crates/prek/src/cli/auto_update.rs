#![allow(clippy::mutable_key_type)]

use std::fmt::Write;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{SystemTime, UNIX_EPOCH};

use annotate_snippets::{AnnotationKind, Level, Renderer, Snippet, renderer::DecorStyle};
use anyhow::{Context, Result};
use futures::{StreamExt, TryStreamExt};
use itertools::Itertools;
use lazy_regex::regex;
use owo_colors::OwoColorize;
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use semver::Version;
use toml_edit::DocumentMut;
use tracing::{debug, trace, warn};

use crate::cli::ExitStatus;
use crate::cli::reporter::AutoUpdateReporter;
use crate::cli::run::Selectors;
use crate::config::{Repo, looks_like_sha};
use crate::fs::{CWD, Simplified};
use crate::printer::Printer;
use crate::run::CONCURRENCY;
use crate::store::Store;
use crate::workspace::{Project, Workspace};
use crate::yaml::serialize_yaml_scalar;
use crate::{config, git};

/// The `rev` value to write back to config, plus an optional `# frozen:` comment.
#[derive(Default, Clone)]
struct Revision {
    /// The resolved revision string to store in `rev`.
    rev: String,
    /// The tag-like reference to preserve in a `# frozen:` comment.
    frozen: Option<String>,
}

/// One occurrence of a remote repo in a project config file.
struct RepoUsage<'a> {
    /// The project whose config contains this repo entry.
    project: &'a Project,
    /// The number of remote repos in that project config.
    remote_count: usize,
    /// The position of this remote repo among the project's remote repos.
    remote_index: usize,
    /// The existing `# frozen:` comment for this repo entry, if present.
    current_frozen: Option<String>,
    /// The source location of the existing `# frozen:` comment, if present.
    current_frozen_site: Option<FrozenCommentSite>,
}

/// One distinct `repo + rev + hook set` target that should be evaluated.
struct RepoTarget<'a> {
    /// The remote repository URL.
    repo: &'a str,
    /// The currently configured `rev` for this target.
    current_rev: &'a str,
    /// The sorted hook ids that must still exist after updating this target.
    required_hook_ids: Vec<&'a str>,
    /// Every config usage that shares this exact `repo + rev + hook set`.
    usages: Vec<RepoUsage<'a>>,
}

/// One fetched remote repository URL with all configured revisions that use it.
struct RepoSource<'a> {
    /// The remote repository URL.
    repo: &'a str,
    /// Distinct configured revisions that should be evaluated against this fetched repo.
    targets: Vec<RepoTarget<'a>>,
}

/// The action to take when a `# frozen:` comment no longer matches a SHA `rev`.
enum FrozenMismatch {
    /// Rewrite the comment to this replacement tag.
    ReplaceWith(String),
    /// Warn only because no tag points at the configured commit.
    NoReplacement,
}

/// One stale `# frozen:` comment found for a specific repo entry.
struct FrozenCommentMismatch<'a> {
    /// The project config that contains this stale comment.
    project: &'a Project,
    /// The number of remote repos in that project config.
    remote_size: usize,
    /// The position of this remote repo among the project's remote repos.
    remote_index: usize,
    /// The current `# frozen:` reference string from config.
    current_frozen: String,
    /// The source location of the current `# frozen:` comment.
    frozen_site: Option<FrozenCommentSite>,
    /// The action to take for this stale comment.
    mismatch: FrozenMismatch,
}

/// The source location of a `# frozen:` comment value within a config line.
#[derive(Clone)]
struct FrozenCommentSite {
    /// The 1-based line number in the config file.
    line_number: usize,
    /// The full source line that contains the `# frozen:` comment.
    source_line: String,
    /// The byte range of the frozen reference value within `source_line`.
    span: Range<usize>,
}

/// Parsed frozen-comment metadata for one `rev` entry in config.
#[derive(Clone)]
struct FrozenRef {
    /// The parsed frozen reference value, if the `rev` line has one.
    current_frozen: Option<String>,
    /// The source location of that frozen reference value, if present.
    site: Option<FrozenCommentSite>,
}

/// A tag reference with the metadata needed for cooldown selection and SHA matching.
struct TagTimestamp {
    /// The tag name without the `refs/tags/` prefix.
    tag: String,
    /// The tag timestamp used for cooldown ordering.
    timestamp: u64,
    /// The peeled commit SHA the tag ultimately points at.
    commit: String,
}

/// The successful result of evaluating one configured `repo + rev + hook set` target.
struct ResolvedRepoUpdate<'a> {
    /// The revision data that may be written back to config.
    revision: Revision,
    /// Any stale `# frozen:` comments found for this target's usages.
    frozen_mismatches: Vec<FrozenCommentMismatch<'a>>,
}

/// The final outcome for one configured `repo + rev + hook set` target.
struct RepoUpdate<'a> {
    /// The target that was evaluated.
    target: &'a RepoTarget<'a>,
    /// The computed result for this target.
    result: Result<ResolvedRepoUpdate<'a>>,
}

/// Pending config mutations grouped by project config file.
type ProjectUpdates<'a> = FxHashMap<&'a Project, Vec<Option<Revision>>>;

/// Updates remote repo revisions and, when possible, keeps existing `# frozen:` comments in sync.
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
    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    // TODO: support selectors?
    let selectors = Selectors::default();
    let workspace = Workspace::discover(store, workspace_root, config, Some(&selectors), true)?;

    let repo_sources = collect_repo_sources(&workspace)?;
    let jobs = if jobs == 0 { *CONCURRENCY } else { jobs };
    let jobs = jobs
        .min(if filter_repos.is_empty() {
            repo_sources.len()
        } else {
            filter_repos.len()
        })
        .max(1);

    let reporter = AutoUpdateReporter::new(printer);

    let sources = repo_sources.iter().filter(|repo_source| {
        filter_repos.is_empty() || filter_repos.iter().any(|repo| repo == repo_source.repo)
    });
    let mut outcomes: Vec<RepoUpdate<'_>> = futures::stream::iter(sources)
        .map(async |repo_source| {
            let progress = reporter.on_update_start(repo_source.repo);
            let result =
                evaluate_repo_source(repo_source, bleeding_edge, freeze, cooldown_days).await;
            reporter.on_update_complete(progress);
            result
        })
        .buffer_unordered(jobs)
        .try_collect::<Vec<_>>()
        .await?
        .into_iter()
        .flatten()
        .collect();

    reporter.on_complete();

    // Sort outcomes by repository URL and revision for consistent output order.
    outcomes.sort_by(|a, b| {
        a.target
            .repo
            .cmp(b.target.repo)
            .then_with(|| a.target.current_rev.cmp(b.target.current_rev))
            .then_with(|| a.target.required_hook_ids.cmp(&b.target.required_hook_ids))
    });

    warn_frozen_mismatches(&outcomes, dry_run, printer)?;

    // Group results by project config file
    let mut project_updates: ProjectUpdates<'_> = FxHashMap::default();
    let failure = apply_repo_updates(outcomes, dry_run, printer, &mut project_updates)?;

    if !dry_run {
        for (project, revisions) in project_updates {
            if revisions.iter().any(Option::is_some) {
                write_new_config(project.config_file(), &revisions).await?;
            }
        }
    }

    if failure {
        return Ok(ExitStatus::Failure);
    }
    Ok(ExitStatus::Success)
}

/// Collects the configured remote repos grouped by fetch source, revision, and hook set.
fn collect_repo_sources(workspace: &Workspace) -> Result<Vec<RepoSource<'_>>> {
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

/// Emits all frozen-comment warnings before the normal update output.
fn warn_frozen_mismatches(
    updates: &[RepoUpdate<'_>],
    dry_run: bool,
    printer: Printer,
) -> Result<()> {
    for update in updates {
        let Ok(resolved) = &update.result else {
            continue;
        };

        for mismatch in &resolved.frozen_mismatches {
            write!(
                printer.stderr(),
                "{}",
                render_frozen_mismatch_warning(
                    update.target.repo,
                    update.target.current_rev,
                    mismatch,
                    dry_run
                )
            )?;
        }
    }
    Ok(())
}

/// Applies evaluated repo outcomes, recording config changes and printing stdout/stderr output.
fn apply_repo_updates<'a>(
    updates: Vec<RepoUpdate<'a>>,
    dry_run: bool,
    printer: Printer,
    project_updates: &mut ProjectUpdates<'a>,
) -> Result<bool> {
    let mut failure = false;

    for update in updates {
        match update.result {
            Ok(resolved) => {
                let is_changed = update.target.current_rev != resolved.revision.rev;
                let has_frozen_notice = !resolved.frozen_mismatches.is_empty();

                // If `rev` itself is unchanged, the normal update path below will not rewrite this
                // repo entry. Still fix stale `# frozen:` comments in update mode so the comment
                // continues to point at the configured commit SHA.
                if !dry_run && !is_changed {
                    for mismatch in &resolved.frozen_mismatches {
                        let FrozenMismatch::ReplaceWith(replacement) = &mismatch.mismatch else {
                            continue;
                        };

                        writeln!(
                            printer.stdout(),
                            "[{}] updating frozen reference {} -> {}",
                            update.target.repo.cyan(),
                            mismatch.current_frozen,
                            replacement,
                        )?;

                        record_project_revision(
                            project_updates,
                            mismatch.project,
                            mismatch.remote_size,
                            mismatch.remote_index,
                            Revision {
                                rev: update.target.current_rev.to_string(),
                                frozen: Some(replacement.clone()),
                            },
                        );
                    }
                }

                if is_changed {
                    writeln!(
                        printer.stdout(),
                        "[{}] {} {} -> {}",
                        update.target.repo.cyan(),
                        if dry_run { "would update" } else { "updating" },
                        update.target.current_rev,
                        resolved.revision.rev
                    )?;

                    for usage in &update.target.usages {
                        record_project_revision(
                            project_updates,
                            usage.project,
                            usage.remote_count,
                            usage.remote_index,
                            resolved.revision.clone(),
                        );
                    }
                } else if !has_frozen_notice {
                    writeln!(
                        printer.stdout(),
                        "[{}] already up to date",
                        update.target.repo.yellow()
                    )?;
                }
            }
            Err(e) => {
                failure = true;
                writeln!(
                    printer.stderr(),
                    "[{}] update failed: {e}",
                    update.target.repo.red()
                )?;
            }
        }
    }

    Ok(failure)
}

fn record_project_revision<'a>(
    project_updates: &mut ProjectUpdates<'a>,
    project: &'a Project,
    remote_size: usize,
    remote_index: usize,
    revision: Revision,
) {
    let revisions = project_updates
        .entry(project)
        .or_insert_with(|| vec![None; remote_size]);
    revisions[remote_index] = Some(revision);
}

/// Collects stale `# frozen:` comments for one configured `repo + rev + hook set` target.
async fn collect_frozen_mismatches<'a>(
    repo_path: &Path,
    target: &'a RepoTarget<'a>,
    tag_timestamps: &[TagTimestamp],
) -> Result<Vec<FrozenCommentMismatch<'a>>> {
    if !looks_like_sha(target.current_rev) {
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

            let mismatch = if let Some(frozen_commit) = frozen_commit.as_deref()
                && frozen_commit.eq_ignore_ascii_case(target.current_rev)
            {
                None
            } else {
                Some(
                    select_best_tag(&rev_tags, current_frozen, true)
                        .map_or(FrozenMismatch::NoReplacement, |tag: &str| {
                            FrozenMismatch::ReplaceWith(tag.to_string())
                        }),
                )
            }?;

            Some(FrozenCommentMismatch {
                project: usage.project,
                remote_size: usage.remote_count,
                remote_index: usage.remote_index,
                current_frozen: current_frozen.to_string(),
                frozen_site: usage.current_frozen_site.clone(),
                mismatch,
            })
        })
        .collect())
}

/// Fetches a remote repository once, then evaluates all configured revisions that use it.
async fn evaluate_repo_source<'a>(
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
        let metadata = load_tag_metadata(repo_source, repo_path, bleeding_edge).await?;

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

/// Loads fetched tag metadata when any revision evaluation depends on it.
async fn load_tag_metadata(
    repo_source: &RepoSource<'_>,
    repo_path: &Path,
    bleeding_edge: bool,
) -> Result<Vec<TagTimestamp>> {
    let should_load_tag_timestamps = repo_source.targets.iter().any(|target| {
        !bleeding_edge
            || (looks_like_sha(target.current_rev)
                && target
                    .usages
                    .iter()
                    .any(|usage| usage.current_frozen.is_some()))
    });

    if should_load_tag_timestamps {
        list_tag_metadata(repo_path).await
    } else {
        Ok(Vec::new())
    }
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
        if rev == exact {
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

/// Initializes a temporary git repo and fetches the remote HEAD plus tags.
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

/// Resolves any revision-like string to the underlying commit SHA.
async fn resolve_revision_to_commit(repo_path: &Path, rev: &str) -> Result<String> {
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

fn get_tags_pointing_at_revision<'a>(
    tag_timestamps: &'a [TagTimestamp],
    rev: &str,
) -> Vec<&'a str> {
    tag_timestamps
        .iter()
        .filter(|tag_timestamp| tag_timestamp.commit.eq_ignore_ascii_case(rev))
        .map(|tag_timestamp| tag_timestamp.tag.as_str())
        .collect()
}

/// Formats one stale `# frozen:` warning as an annotated source snippet.
fn render_frozen_mismatch_warning(
    repo: &str,
    current_rev: &str,
    mismatch: &FrozenCommentMismatch<'_>,
    dry_run: bool,
) -> String {
    let details = match &mismatch.mismatch {
        FrozenMismatch::ReplaceWith(replacement) => {
            if dry_run {
                format!("would update comment to `{replacement}`")
            } else {
                format!("updating comment to `{replacement}`")
            }
        }
        FrozenMismatch::NoReplacement => "no tag points at the configured commit".to_string(),
    };
    let title = format!("[{repo}] frozen comment does not match `rev: {current_rev}`");

    let site = mismatch
        .frozen_site
        .as_ref()
        .expect("frozen comment site must exist when rendering a frozen mismatch warning");
    let report = Level::WARNING
        .primary_title(title)
        .element(
            Snippet::source(&site.source_line)
                .line_start(site.line_number)
                .path(mismatch.project.config_file().user_display().to_string())
                .annotation(
                    AnnotationKind::Primary
                        .span(site.span.clone())
                        .label(format!(
                            "`{}` resolves to a different commit",
                            mismatch.current_frozen
                        )),
                ),
        )
        .element(Level::NOTE.message(details));

    let renderer = Renderer::styled().decor_style(DecorStyle::Unicode);
    format!("{}\n", renderer.render(&[report]))
}

fn parse_frozen_ref(line: &str, line_number: usize) -> FrozenRef {
    let Some(captures) = regex!(r#"#\s*frozen:\s*([^\s#]+)"#).captures(line) else {
        return FrozenRef {
            current_frozen: None,
            site: None,
        };
    };
    let frozen_match = captures.get(1).expect("capture group 1 must exist");
    FrozenRef {
        current_frozen: Some(frozen_match.as_str().to_string()),
        site: Some(FrozenCommentSite {
            line_number,
            source_line: line.to_string(),
            span: frozen_match.start()..frozen_match.end(),
        }),
    }
}

fn read_frozen_refs(path: &Path) -> Result<Vec<FrozenRef>> {
    let content = fs_err::read_to_string(path)?;

    match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("toml") => Ok(content
            .lines()
            .enumerate()
            .filter(|(_, line)| regex!(r#"^\s*rev\s*="#).is_match(line))
            .map(|(index, line)| parse_frozen_ref(line, index + 1))
            .collect()),
        _ => {
            let rev_regex = regex!(r#"^\s+rev:\s*['"]?[^\s#]+(?P<comment>.*)$"#);
            Ok(content
                .lines()
                .enumerate()
                .filter_map(|(index, line)| {
                    rev_regex
                        .captures(line)
                        .map(|_| parse_frozen_ref(line, index + 1))
                })
                .collect())
        }
    }
}

/// Resolves the default branch tip to an exact tag when possible, otherwise to a commit SHA.
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

/// Lists fetched tag metadata sorted from newest to oldest timestamp.
///
/// Within groups of tags sharing the same timestamp, semver-parseable tags
/// are sorted highest version first; non-semver tags sort after them.
async fn list_tag_metadata(repo: &Path) -> Result<Vec<TagTimestamp>> {
    let output = git::git_cmd("git for-each-ref")?
        .arg("for-each-ref")
        .arg("--sort=-creatordate")
        // `creatordate` is the date the tag was created (annotated tags) or the commit date (lightweight tags)
        // `lstrip=2` removes the "refs/tags/" prefix
        // `objectname` is the tag object SHA for annotated tags, while `*objectname`
        // peels annotated tags to their target object. For lightweight tags the peeled
        // value is empty, so we fall back to `objectname`.
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
            Some(TagTimestamp {
                tag: tag.to_string(),
                timestamp: ts,
                commit: commit.to_string(),
            })
        })
        .collect();

    // Deterministic sort: primary key is timestamp (newest first).
    // Within equal timestamps, prefer higher semver versions; non-semver tags
    // sort after semver ones. As a final tie-breaker, compare the tag refname
    // so ordering is stable across platforms/filesystems.
    tags.sort_by(|tag_a, tag_b| {
        tag_b.timestamp.cmp(&tag_a.timestamp).then_with(|| {
            let ver_a = Version::parse(tag_a.tag.strip_prefix('v').unwrap_or(&tag_a.tag));
            let ver_b = Version::parse(tag_b.tag.strip_prefix('v').unwrap_or(&tag_b.tag));
            match (ver_a, ver_b) {
                (Ok(a), Ok(b)) => b.cmp(&a).then_with(|| tag_a.tag.cmp(&tag_b.tag)),
                (Ok(_), Err(_)) => std::cmp::Ordering::Less,
                (Err(_), Ok(_)) => std::cmp::Ordering::Greater,
                (Err(_), Err(_)) => tag_a.tag.cmp(&tag_b.tag),
            }
        })
    });

    Ok(tags)
}

/// Selects the revision string that `auto-update` should write for one fetched repo target.
///
/// In normal mode this chooses the newest tag that satisfies the cooldown window.
/// In bleeding-edge mode it resolves `FETCH_HEAD` instead.
async fn select_update_revision(
    repo_path: &Path,
    current_rev: &str,
    bleeding_edge: bool,
    cooldown_days: u8,
    tag_timestamps: &[TagTimestamp],
) -> Result<Option<String>> {
    if bleeding_edge {
        return resolve_bleeding_edge(repo_path).await;
    }

    let cutoff_secs = u64::from(cooldown_days) * 86400;
    let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
    let cutoff = now.saturating_sub(cutoff_secs);

    // `tag_timestamps` is sorted newest -> oldest; find the first bucket where ts <= cutoff.
    let left = match tag_timestamps.binary_search_by(|tag| tag.timestamp.cmp(&cutoff).reverse()) {
        Ok(i) | Err(i) => i,
    };

    let Some(target_tag) = tag_timestamps.get(left) else {
        trace!("No tags meet cooldown cutoff {cutoff_secs}s");
        return Ok(None);
    };

    debug!(
        "Using tag `{}` cutoff timestamp {}",
        target_tag.tag, target_tag.timestamp
    );

    let tags = get_tags_pointing_at_revision(tag_timestamps, &target_tag.commit);
    let best = select_best_tag(&tags, current_rev, false)
        .unwrap_or(target_tag.tag.as_str())
        .to_string();
    debug!(
        "Using best candidate tag `{best}` for revision `{}`",
        target_tag.tag
    );

    Ok(Some(best))
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
fn select_best_tag<'a>(
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
async fn checkout_and_validate_manifest(
    repo_path: &Path,
    rev: &str,
    required_hook_ids: &[&str],
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

/// Rewrites one config file with the resolved revisions for its remote repos.
async fn write_new_config(path: &Path, revisions: &[Option<Revision>]) -> Result<()> {
    let content = fs_err::tokio::read_to_string(path).await?;
    let new_content = match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("toml") => {
            render_updated_toml_config(path, &content, revisions)?
        }
        _ => render_updated_yaml_config(path, &content, revisions)?,
    };

    fs_err::tokio::write(path, new_content)
        .await
        .with_context(|| {
            format!(
                "Failed to write updated config file `{}`",
                path.user_display()
            )
        })?;

    Ok(())
}

/// Updates `rev` values and `# frozen:` comments in a TOML config while preserving formatting.
fn render_updated_toml_config(
    path: &Path,
    content: &str,
    revisions: &[Option<Revision>],
) -> Result<String> {
    let mut doc = content.parse::<DocumentMut>()?;
    let Some(repos) = doc
        .get_mut("repos")
        .and_then(|item| item.as_array_of_tables_mut())
    else {
        anyhow::bail!("Missing `[[repos]]` array in `{}`", path.user_display());
    };

    let mut remote_repos = Vec::new();
    for table in repos.iter_mut() {
        let repo_value = table
            .get("repo")
            .and_then(|item| item.as_value())
            .and_then(|value| value.as_str())
            .unwrap_or_default();

        if matches!(repo_value, "local" | "meta" | "builtin") {
            continue;
        }

        if !table.contains_key("rev") {
            anyhow::bail!(
                "Found remote repo without `rev` in `{}`",
                path.user_display()
            );
        }

        remote_repos.push(table);
    }

    if remote_repos.len() != revisions.len() {
        anyhow::bail!(
            "Found {} remote repos in `{}` but expected {}, file content may have changed",
            remote_repos.len(),
            path.user_display(),
            revisions.len()
        );
    }

    for (table, revision) in remote_repos.into_iter().zip_eq(revisions) {
        let Some(revision) = revision else {
            continue;
        };

        let Some(value) = table.get_mut("rev").and_then(|item| item.as_value_mut()) else {
            continue;
        };

        let suffix = value
            .decor()
            .suffix()
            .and_then(|s| s.as_str())
            .filter(|s| !s.trim_start().starts_with("# frozen:"))
            .map(str::to_string);

        *value = toml_edit::Value::from(revision.rev.clone());

        if let Some(frozen) = &revision.frozen {
            value.decor_mut().set_suffix(format!(" # frozen: {frozen}"));
        } else if let Some(suffix) = suffix {
            value.decor_mut().set_suffix(suffix);
        }
    }

    Ok(doc.to_string())
}

/// Updates `rev` values and `# frozen:` comments in a YAML config while preserving line layout.
fn render_updated_yaml_config(
    path: &Path,
    content: &str,
    revisions: &[Option<Revision>],
) -> Result<String> {
    let mut lines = content
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

        let caps = rev_regex
            .captures(&lines[*line_no])
            .context("Failed to capture rev line")?;

        let new_rev = serialize_yaml_scalar(&revision.rev, &caps[3])?;

        let comment = if let Some(frozen) = &revision.frozen {
            format!("  # frozen: {frozen}")
        } else if caps[5].trim_start().starts_with("# frozen:") {
            String::new()
        } else {
            caps[5].to_string()
        };

        lines[*line_no] = format!(
            "{}rev:{}{}{}{}",
            &caps[1], &caps[2], new_rev, comment, &caps[6]
        );
    }

    Ok(lines.join(""))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::Cmd;
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

    fn get_backdated_timestamp(days_ago: u64) -> u64 {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        now - (days_ago * 86400)
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

        create_backdated_commit(repo, "candidate", 5).await;
        create_lightweight_tag(repo, "v2.0.0-rc1").await;
        create_lightweight_tag(repo, "totally-different").await;

        create_backdated_commit(repo, "latest", 1).await;
        create_lightweight_tag(repo, "v2.0.0").await;

        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev = select_update_revision(repo, "v2.0.0", false, 3, &tag_timestamps)
            .await
            .unwrap();

        assert_eq!(rev, Some("v2.0.0-rc1".to_string()));
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
        let rev = select_update_revision(repo, "v1.1.0", false, 5, &tag_timestamps)
            .await
            .unwrap();

        assert_eq!(rev, None);
    }

    #[tokio::test]
    async fn test_select_update_revision_picks_oldest_eligible_bucket() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "oldest", 10).await;
        create_lightweight_tag(repo, "v1.0.0").await;

        create_backdated_commit(repo, "mid", 4).await;
        create_lightweight_tag(repo, "v1.1.0").await;

        create_backdated_commit(repo, "newest", 1).await;
        create_lightweight_tag(repo, "v1.2.0").await;

        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev = select_update_revision(repo, "v1.2.0", false, 5, &tag_timestamps)
            .await
            .unwrap();

        assert_eq!(rev, Some("v1.0.0".to_string()));
    }

    #[tokio::test]
    async fn test_select_update_revision_prefers_version_like_tags() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        create_backdated_commit(repo, "eligible", 2).await;
        create_lightweight_tag(repo, "moving-tag").await;
        create_lightweight_tag(repo, "v1.0.0").await;

        // Even though the current rev matches the moving tag exactly, the dotted tag
        // should be preferred.
        let tag_timestamps = list_tag_metadata(repo).await.unwrap();
        let rev = select_update_revision(repo, "moving-tag", false, 1, &tag_timestamps)
            .await
            .unwrap();

        assert_eq!(rev, Some("v1.0.0".to_string()));
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
        let rev = select_update_revision(repo, "v1.2.3", false, 1, &tag_timestamps)
            .await
            .unwrap();

        assert_eq!(rev, Some("v1.2.0".to_string()));
    }

    #[tokio::test]
    async fn test_list_tag_metadata_stable_order_for_equal_timestamps() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        // Create multiple tags on the same commit (same timestamp)
        create_backdated_commit(repo, "release", 5).await;
        create_lightweight_tag(repo, "v1.0.0").await;
        create_lightweight_tag(repo, "v1.0.3").await;
        create_lightweight_tag(repo, "v1.0.5").await;
        create_lightweight_tag(repo, "v1.0.2").await;

        let timestamps = list_tag_metadata(repo).await.unwrap();

        // All timestamps are equal (tags on same commit).
        // Within equal timestamps, semver tags should sort highest version first.
        let tags: Vec<&str> = timestamps.iter().map(|tag| tag.tag.as_str()).collect();
        assert_eq!(tags, vec!["v1.0.5", "v1.0.3", "v1.0.2", "v1.0.0"]);
    }

    #[tokio::test]
    async fn test_list_tag_metadata_deterministic_order_for_equal_timestamp_non_semver() {
        let tmp = setup_test_repo().await;
        let repo = tmp.path();

        // Lightweight tags on the same commit share a timestamp.
        create_backdated_commit(repo, "release", 5).await;
        create_lightweight_tag(repo, "beta").await;
        create_lightweight_tag(repo, "alpha").await;
        create_lightweight_tag(repo, "gamma").await;

        let timestamps = list_tag_metadata(repo).await.unwrap();
        let tags: Vec<&str> = timestamps.iter().map(|tag| tag.tag.as_str()).collect();
        assert_eq!(tags, vec!["alpha", "beta", "gamma"]);
    }
}
