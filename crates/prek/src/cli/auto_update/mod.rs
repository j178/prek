use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::Result;
use futures::{StreamExt, TryStreamExt};
use rustc_hash::FxHashMap;

use crate::cli::ExitStatus;
use crate::cli::auto_update::config::write_new_config;
use crate::cli::auto_update::display::{apply_repo_updates, warn_frozen_mismatches};
use crate::cli::auto_update::source::{collect_repo_sources, evaluate_repo_source};
use crate::cli::reporter::AutoUpdateReporter;
use crate::cli::run::Selectors;
use crate::fs::CWD;
use crate::printer::Printer;
use crate::run::CONCURRENCY;
use crate::store::Store;
use crate::workspace::{Project, Workspace};

mod config;
mod display;
mod repository;
mod source;

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
    /// The 1-based line number of this repo entry's `rev` setting.
    rev_line_number: usize,
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
enum FrozenMismatchAction {
    /// Rewrite the comment to this replacement tag.
    ReplaceWith(String),
    /// Remove the stale comment because no ref points at the pinned commit.
    Remove,
    /// Warn only because we cannot safely decide a comment-only fix.
    NoReplacement,
}

/// Whether the pinned SHA is available from the refs fetched for `auto-update`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CommitPresence {
    /// The commit is present in the fetched repository view.
    Present,
    /// The commit is not present in the fetched repository view.
    Absent,
    /// The current Git cannot disable lazy fetch, so presence could not be checked safely.
    Unknown,
}

/// Why an existing `# frozen:` comment no longer matches the configured `rev`.
enum FrozenMismatchReason {
    /// The frozen reference resolves successfully, but to a different commit than `rev`.
    ResolvesToDifferentCommit,
    /// The frozen reference could not be resolved at all.
    Unresolvable,
}

/// One stale `# frozen:` comment found for a specific repo entry.
struct FrozenMismatch<'a> {
    /// The project config that contains this stale comment.
    project: &'a Project,
    /// The number of remote repos in that project config.
    remote_size: usize,
    /// The position of this remote repo among the project's remote repos.
    remote_index: usize,
    /// The 1-based line number of the `rev` setting that owns this stale comment.
    rev_line_number: usize,
    /// The current `# frozen:` reference string from config.
    current_frozen: String,
    /// The source location of the current `# frozen:` comment.
    frozen_site: Option<FrozenCommentSite>,
    /// Why the existing frozen reference is stale.
    reason: FrozenMismatchReason,
    /// Whether the pinned SHA is available in the fetched repository view.
    current_rev_presence: CommitPresence,
    /// The action to take for this stale comment.
    action: FrozenMismatchAction,
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
    /// The 1-based line number of the `rev` setting.
    line_number: usize,
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
    frozen_mismatches: Vec<FrozenMismatch<'a>>,
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

struct ApplyRepoUpdatesResult {
    failure: bool,
    has_updates: bool,
}

enum DisplayEventKind {
    Update { current: Revision, next: Revision },
    FrozenUpdate { current: String, next: String },
    FrozenRemove { current: String },
    UpToDate { current: Revision },
    Failure { error: String },
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum DisplayStream {
    Stdout,
    Stderr,
}

struct DisplayEvent<'a> {
    stream: DisplayStream,
    project: &'a Project,
    repo: &'a str,
    remote_index: usize,
    line_number: usize,
    kind: DisplayEventKind,
}

struct FrozenWarningEvent<'a> {
    project: &'a Project,
    repo: &'a str,
    current_rev: &'a str,
    remote_index: usize,
    mismatch: &'a FrozenMismatch<'a>,
}

type RepoOccurrences<'a> = FxHashMap<(&'a Path, &'a str), usize>;

/// Updates remote repo revisions and, when possible, keeps existing `# frozen:` comments in sync.
#[expect(clippy::fn_params_excessive_bools)]
pub(crate) async fn auto_update(
    store: &Store,
    config: Option<PathBuf>,
    filter_repos: Vec<String>,
    exclude_repos: Vec<String>,
    verbose: bool,
    bleeding_edge: bool,
    freeze: bool,
    jobs: usize,
    dry_run: bool,
    check: bool,
    cooldown_days: u8,
    printer: Printer,
) -> Result<ExitStatus> {
    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    // TODO: support selectors?
    let selectors = Selectors::default();
    let workspace = Workspace::discover(store, workspace_root, config, Some(&selectors), true)?;
    let jobs = if jobs == 0 { *CONCURRENCY } else { jobs };
    let reporter = AutoUpdateReporter::new(printer);

    let repo_sources = collect_repo_sources(&workspace)?;
    let sources = repo_sources.iter().filter(|repo_source| {
        (filter_repos.is_empty() || filter_repos.iter().any(|repo| repo == repo_source.repo))
            && !exclude_repos.iter().any(|repo| repo == repo_source.repo)
    });
    let outcomes: Vec<RepoUpdate<'_>> = futures::stream::iter(sources)
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

    warn_frozen_mismatches(&outcomes, printer)?;

    // Group results by project config file
    #[expect(clippy::mutable_key_type)]
    let mut project_updates: ProjectUpdates<'_> = FxHashMap::default();
    let apply_result =
        apply_repo_updates(outcomes, verbose, dry_run, printer, &mut project_updates)?;

    if !dry_run {
        for (project, revisions) in project_updates {
            if revisions.iter().any(Option::is_some) {
                write_new_config(project.config_file(), &revisions).await?;
            }
        }
    }

    if apply_result.failure || (check && apply_result.has_updates) {
        return Ok(ExitStatus::Failure);
    }
    Ok(ExitStatus::Success)
}
