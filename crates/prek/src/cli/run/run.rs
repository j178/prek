use std::fmt::Write as _;
use std::io::Write as _;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::slice;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use asyncband::semaphore::Semaphore;
use futures_util::TryStreamExt;
use futures_util::stream::FuturesUnordered;
use owo_colors::{OwoColorize, Styled};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PREK_TOML};
use prek_identify::{TagSet, tags_from_path};
use rustc_hash::{FxBuildHasher, FxHashMap};
use tracing::{debug, error, trace};
use unicode_width::UnicodeWidthStr;

use crate::cli::reporter::{HookInitReporter, HookInstallReporter};
use crate::cli::run::diff::DiffTracker;
use crate::cli::run::filter::{RunInputMode, stage_uses_message_file_input};
use crate::cli::run::install::{InstallCache, install_hooks};
use crate::cli::run::keeper::WorkTreeKeeper;
use crate::cli::run::{
    CollectOptions, FileSelection, FileTagCache, GroupFilters, HookFileFilter, HookRunReporter,
    ProjectFiles, RunFileIndex, RunInput, Selectors, collect_run_input, project_status_marker,
};
use crate::cli::{ExitStatus, RunArgs, RunExtraArgs, RunOptions, flag};
use crate::config::{PassFilenames, Stage};
use crate::fs::CWD;
use crate::git::GIT_ROOT;
use crate::hook::{Hook, InstalledHook};
use crate::printer::Printer;
use crate::run::HOOK_CONCURRENCY;
use crate::store::Store;
use crate::terminal::{USE_COLOR, sanitize_output};
use crate::workspace::{HookInitFilters, Project, Workspace};
use crate::{fs, git, hooks, warn_user};

use super::selector::HookSelection;
use super::{DRY_RUN, FAILED, PASSED, SKIPPED};

#[derive(Clone)]
enum HookPlan<T> {
    Run(T),
    Skip(Arc<Hook>),
}

impl<T> HookPlan<T> {
    fn as_run(&self) -> Option<&T> {
        match self {
            Self::Run(hook) => Some(hook),
            Self::Skip(_) => None,
        }
    }
}

impl<T> Deref for HookPlan<T>
where
    T: Deref<Target = Hook>,
{
    type Target = Hook;

    fn deref(&self) -> &Self::Target {
        match self {
            Self::Run(hook) => hook,
            Self::Skip(hook) => hook,
        }
    }
}

type SelectedHook = HookPlan<Arc<Hook>>;
type ScheduledHook = HookPlan<InstalledHook>;

pub(crate) async fn run(
    store: &Store,
    config: Option<PathBuf>,
    args: RunArgs,
    refresh: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let RunArgs {
        options,
        stage: hook_stage,
        groups,
        required_groups,
        no_groups,
    } = args;
    let RunOptions {
        includes,
        skips,
        file_selection,
        show_diff_on_failure,
        fail_fast,
        no_fail_fast,
        dry_run,
        hide_status,
        extra: extra_args,
    } = options;
    let selection: FileSelection = file_selection.into();
    let fail_fast = flag(fail_fast, no_fail_fast);

    // Prevent recursive post-checkout hooks.
    if hook_stage == Some(Stage::PostCheckout)
        && EnvVars.is_set(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT)
    {
        return Ok(ExitStatus::Success);
    }

    // Ensure we are in a git repository.
    LazyLock::force(&GIT_ROOT).as_ref()?;

    let should_stash = selection.requires_clean_worktree();

    // Check if we have unresolved merge conflict files and fail fast.
    if should_stash && git::has_unmerged_paths().await? {
        anyhow::bail!(
            "Found unresolved merge conflicts. Resolve the conflicts, stage the files with `git add`, and try again"
        );
    }

    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    let selectors = Selectors::load(&includes, &skips, &workspace_root)?;
    let group_filters = GroupFilters::parse(&groups, &required_groups, &no_groups)?;
    let has_group_filters = group_filters.has_filters();
    let workspace = Workspace::discover(store, workspace_root, config, Some(&selectors), refresh)?;

    if should_stash {
        workspace.check_configs_staged().await?;
    }

    let reporter = HookInitReporter::new(printer);
    let hooks = {
        let _lock = store.lock_async().await?;
        store.track_configs(workspace.config_files())?;

        workspace
            .init_hooks(
                store,
                HookInitFilters::new(Some(&selectors), Some(&group_filters)),
                Some(&reporter),
            )
            .await
            .context("Failed to init hooks")?
    };
    let mut selected_hooks = Vec::new();
    for hook in hooks {
        let hook = match selectors.select_hook(&hook) {
            HookSelection::Selected => HookPlan::Run(Arc::new(hook)),
            HookSelection::Skipped => HookPlan::Skip(Arc::new(hook)),
            HookSelection::NotSelected => continue,
        };
        if group_filters.matches_hook(&hook) {
            selected_hooks.push(hook);
        }
    }

    selectors.report_unused();
    group_filters.report_unused();

    if selected_hooks.is_empty() {
        writeln!(
            printer.stderr(),
            "{}: No hooks found after filtering with the given selectors",
            "error".red().bold(),
        )?;
        if selectors.has_project_selectors() {
            writeln!(
                printer.stderr(),
                "\n{} If you just added a new `{}` or `{}`, try rerunning your command with the `{}` flag to rescan the workspace.",
                "hint:".bold().yellow(),
                PREK_TOML.cyan(),
                PRE_COMMIT_CONFIG_YAML.cyan(),
                "--refresh".cyan(),
            )?;
        }
        return Ok(ExitStatus::Failure);
    }

    let (stage_filter, input_mode) =
        infer_stage_and_input_mode(hook_stage, has_group_filters, &selected_hooks, &selectors);
    if let Some(stage_filter) = stage_filter {
        selected_hooks.retain(|h| h.stages.contains(stage_filter));
    } else {
        // Group selection without an explicit stage uses normal file input, so
        // hooks that can only consume Git message files cannot run correctly.
        selected_hooks.retain(|hook| !uses_only_message_file_input(hook));
    }

    if selected_hooks.is_empty() {
        if let Some(stage) = stage_filter {
            debug!("No hooks found for stage {stage} after filtering, exit early");
        } else {
            warn_user!(
                "all hooks selected by group filters require `commit-msg` or `prepare-commit-msg` stage and were not run; pass `--stage commit-msg` or `--stage prepare-commit-msg` to run them"
            );
            return Ok(ExitStatus::Failure);
        }
        return Ok(ExitStatus::Success);
    }

    debug!(
        "Hooks going to run: {:?}",
        selected_hooks.iter().map(|h| &h.id).collect::<Vec<_>>()
    );

    // Clear any unstaged changes from the git working directory.
    let mut _guard = None;
    if should_stash {
        _guard = Some(
            WorkTreeKeeper::clean(store, workspace.root())
                .await
                .context("Failed to clean work tree")?,
        );
    }

    let (from_ref, to_ref) = selection.refs();
    set_env_vars(from_ref, to_ref, &extra_args);

    let input = collect_run_input(
        workspace.root(),
        CollectOptions {
            input_mode,
            selection,
            commit_msg_filename: extra_args.commit_msg_filename,
        },
    )
    .await
    .context("Failed to collect files")?;

    // Change to the workspace root directory.
    std::env::set_current_dir(workspace.root()).with_context(|| {
        format!(
            "Failed to change directory to `{}`",
            workspace.root().display()
        )
    })?;

    let file_index = RunFileIndex::new(&input, workspace.all_projects());
    let installed_hooks = ensure_hooks_installed(
        store,
        printer,
        &workspace,
        &input,
        &file_index,
        selected_hooks,
    )
    .await?;

    run_hooks(
        &workspace,
        &input,
        &file_index,
        &installed_hooks,
        store,
        show_diff_on_failure,
        fail_fast,
        dry_run,
        &hide_status,
        should_stash,
        verbose,
        printer,
    )
    .await
}

fn infer_stage_and_input_mode(
    explicit_stage: Option<Stage>,
    has_group_filters: bool,
    selected_hooks: &[SelectedHook],
    selectors: &Selectors,
) -> (Option<Stage>, RunInputMode) {
    if let Some(stage) = explicit_stage {
        return (Some(stage), RunInputMode::from(stage));
    }

    if has_group_filters {
        return (None, RunInputMode::Files);
    }

    // Preserve legacy direct-hook execution: try `manual` only when the user
    // named hooks directly and none of those hooks can run as `pre-commit`.
    let has_runnable_pre_commit_hook = selected_hooks
        .iter()
        .filter_map(HookPlan::as_run)
        .any(|hook| hook.stages.contains(Stage::PreCommit));
    let stage = if selectors.includes_only_hook_targets() && !has_runnable_pre_commit_hook {
        Stage::Manual
    } else {
        Stage::PreCommit
    };
    (Some(stage), RunInputMode::from(stage))
}

fn uses_only_message_file_input(hook: &Hook) -> bool {
    !hook.stages.is_empty() && hook.stages.iter().all(stage_uses_message_file_input)
}

// `pre-commit` sets these environment variables for other git hooks.
fn set_env_vars(from_ref: Option<&str>, to_ref: Option<&str>, args: &RunExtraArgs) {
    unsafe {
        std::env::set_var("PRE_COMMIT", "1");

        if let Some(source) = &args.prepare_commit_message_source {
            std::env::set_var("PRE_COMMIT_COMMIT_MSG_SOURCE", source);
        }
        if let Some(object) = &args.commit_object_name {
            std::env::set_var("PRE_COMMIT_COMMIT_OBJECT_NAME", object);
        }
        if let Some(from_ref) = from_ref {
            std::env::set_var("PRE_COMMIT_ORIGIN", from_ref);
            std::env::set_var("PRE_COMMIT_FROM_REF", from_ref);
        }
        if let Some(to_ref) = to_ref {
            std::env::set_var("PRE_COMMIT_SOURCE", to_ref);
            std::env::set_var("PRE_COMMIT_TO_REF", to_ref);
        }
        if let Some(upstream) = &args.pre_rebase_upstream {
            std::env::set_var("PRE_COMMIT_PRE_REBASE_UPSTREAM", upstream);
        }
        if let Some(branch) = &args.pre_rebase_branch {
            std::env::set_var("PRE_COMMIT_PRE_REBASE_BRANCH", branch);
        }
        if let Some(branch) = &args.local_branch {
            std::env::set_var("PRE_COMMIT_LOCAL_BRANCH", branch);
        }
        if let Some(branch) = &args.remote_branch {
            std::env::set_var("PRE_COMMIT_REMOTE_BRANCH", branch);
        }
        if let Some(name) = &args.remote_name {
            std::env::set_var("PRE_COMMIT_REMOTE_NAME", name);
        }
        if let Some(url) = &args.remote_url {
            std::env::set_var("PRE_COMMIT_REMOTE_URL", url);
        }
        if let Some(checkout) = &args.checkout_type {
            std::env::set_var("PRE_COMMIT_CHECKOUT_TYPE", checkout);
        }
        if args.is_squash_merge {
            std::env::set_var("PRE_COMMIT_SQUASH_MERGE", "1");
        }
        if let Some(command) = &args.rewrite_command {
            std::env::set_var("PRE_COMMIT_REWRITE_COMMAND", command);
        }
    }
}

/// Ensure installable hooks have environments and return the form expected by the runner.
///
/// Hooks that do not need an environment are returned as-is. Hooks that will be skipped,
/// either explicitly or because they have no matching input, are returned without resolving
/// an environment; `run_hook` reports them before trying to execute them.
async fn ensure_hooks_installed<'paths>(
    store: &Store,
    printer: Printer,
    workspace: &Workspace,
    input: &'paths RunInput,
    file_index: &RunFileIndex<'paths>,
    hooks: Vec<SelectedHook>,
) -> Result<Vec<ScheduledHook>> {
    let runnable_env_hooks = select_runnable_env_hooks(workspace, input, file_index, &hooks)?;
    let mut installed_by_hook = FxHashMap::default();

    if !runnable_env_hooks.is_empty() {
        let _lock = store.lock_async().await?;
        let mut install_cache = InstallCache::new();
        let mut missing_env_hooks = Vec::new();

        for hook in runnable_env_hooks {
            if let Some(installed_hook) = install_cache.installed_hook(store, hook.clone()).await {
                installed_by_hook.insert(hook.key(), installed_hook);
            } else {
                missing_env_hooks.push(hook.clone());
            }
        }

        if !missing_env_hooks.is_empty() {
            let reporter = HookInstallReporter::new(printer);
            let installed_hooks =
                install_hooks(missing_env_hooks, store, &reporter, &mut install_cache).await?;
            reporter.on_complete();

            for installed_hook in installed_hooks {
                installed_by_hook.insert(installed_hook.key(), installed_hook);
            }
        }
    }

    Ok(hooks
        .into_iter()
        .map(|hook| match hook {
            HookPlan::Run(hook) => HookPlan::Run(
                installed_by_hook
                    .remove(&hook.key())
                    .unwrap_or_else(|| InstalledHook::NoNeedInstall(hook)),
            ),
            HookPlan::Skip(hook) => HookPlan::Skip(hook),
        })
        .collect())
}

/// Return installable hooks that should run for this input.
///
/// Filtering happens before consulting the install cache so skipped hooks do not
/// scan or health-check environments that they cannot use in this run.
fn select_runnable_env_hooks<'paths>(
    workspace: &Workspace,
    input: &'paths RunInput,
    file_index: &RunFileIndex<'paths>,
    hooks: &[SelectedHook],
) -> Result<Vec<Arc<Hook>>> {
    #[allow(clippy::mutable_key_type)]
    let mut project_to_hooks: FxHashMap<&Project, Vec<Arc<Hook>>> =
        FxHashMap::with_capacity_and_hasher(workspace.all_projects().len(), FxBuildHasher);
    for hook in hooks.iter().filter_map(HookPlan::as_run) {
        if !hook.needs_install_env() {
            continue;
        }
        project_to_hooks
            .entry(hook.project())
            .or_default()
            .push(hook.clone());
    }

    let mut runnable_env_hooks = Vec::with_capacity(hooks.len());
    let tag_cache = file_index.tag_cache();

    for project in workspace.all_projects() {
        match input {
            RunInput::Files(_) => {
                let Some(mut hooks) = project_to_hooks.remove(project.as_ref()) else {
                    continue;
                };

                let project_files = file_index.project_files(project);
                hooks.retain(|hook| {
                    hook.always_run || project_files.has_matching_file(hook, tag_cache)
                });
                runnable_env_hooks.extend(hooks);
            }
            RunInput::MessageFile(_) => {
                let Some(hooks) = project_to_hooks.remove(project.as_ref()) else {
                    continue;
                };

                let project_input = ProjectHookInput::new(input, project, file_index)?;
                for hook in hooks {
                    if hook.always_run || project_input.matches_hook(&hook, tag_cache) {
                        runnable_env_hooks.push(hook);
                    }
                }
            }
        }
    }

    Ok(runnable_env_hooks)
}

#[allow(clippy::fn_params_excessive_bools)]
async fn run_hooks<'paths>(
    workspace: &Workspace,
    input: &'paths RunInput,
    file_index: &RunFileIndex<'paths>,
    hooks: &[ScheduledHook],
    store: &Store,
    show_diff_on_failure: bool,
    fail_fast: Option<bool>,
    dry_run: bool,
    hide_status: &[HideStatus],
    worktree_cleaned: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    debug_assert!(!hooks.is_empty(), "No hooks to run");

    // Group hooks by project to run them in order of their depth in the workspace.
    #[allow(clippy::mutable_key_type)]
    let mut project_to_hooks: FxHashMap<&Project, Vec<ScheduledHook>> =
        FxHashMap::with_capacity_and_hasher(hooks.len(), FxBuildHasher);
    for hook in hooks {
        project_to_hooks
            .entry(hook.project())
            .or_default()
            .push(hook.clone());
    }

    let show_project_headers =
        project_to_hooks.len() > 1 || project_to_hooks.keys().any(|project| !project.is_root());
    let mut session = HookRunSession::new(
        hooks,
        store,
        dry_run,
        hide_status,
        verbose,
        show_project_headers,
        printer,
    );

    for projects in ProjectDepthGroups::new(workspace.all_projects()) {
        let clean_baseline = worktree_cleaned && !session.modified_files;
        let mut project_runs = Vec::new();

        for project in projects {
            let Some(mut hooks) = project_to_hooks.remove(project.as_ref()) else {
                continue;
            };

            // Sort hooks by priority (lower number means higher priority).
            // If two hooks have the same priority, preserve their original order from the config.
            hooks.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.idx.cmp(&b.idx)));

            project_runs.push(ProjectRun {
                project,
                project_fail_fast: fail_fast
                    .or_else(|| project.config().fail_fast)
                    .unwrap_or(false),
                groups: PriorityGroups::new(hooks).collect(),
            });
        }

        if project_runs.is_empty() {
            continue;
        }

        let project_results = session
            .run_project_level(project_runs, input, file_index, clean_baseline)
            .await?;
        let mut stop_after_level = false;

        for project_result in project_results {
            stop_after_level |= session.finish_project_run(project_result, show_project_headers)?;
        }

        if stop_after_level {
            break;
        }
    }

    session.finish(workspace, show_diff_on_failure).await
}

struct ProjectDepthGroups<'a> {
    projects: &'a [Arc<Project>],
    idx: usize,
}

impl<'a> ProjectDepthGroups<'a> {
    fn new(projects: &'a [Arc<Project>]) -> Self {
        Self { projects, idx: 0 }
    }
}

impl<'a> Iterator for ProjectDepthGroups<'a> {
    type Item = &'a [Arc<Project>];

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.projects.get(self.idx)?;
        let depth = first.depth();
        let start = self.idx;

        while self
            .projects
            .get(self.idx)
            .is_some_and(|project| project.depth() == depth)
        {
            self.idx += 1;
        }

        Some(&self.projects[start..self.idx])
    }
}

struct ProjectRun<'project> {
    project: &'project Project,
    project_fail_fast: bool,
    groups: Vec<Vec<ScheduledHook>>,
}

struct ProjectRunResult<'project> {
    project: &'project Project,
    groups: Vec<PriorityGroupResult>,
    stop_after_level: bool,
}

impl ProjectRunResult<'_> {
    fn failed(&self) -> bool {
        self.groups.iter().any(PriorityGroupResult::failed)
    }

    fn has_visible_report(&self, filter: ReportFilter<'_>) -> bool {
        self.groups
            .iter()
            .any(|group| group.has_visible_report(filter))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum ModificationScope {
    SingleHook,
    PriorityGroup,
}

impl ModificationScope {
    fn from_results(results: &mut [HookRunResult], modified_files: bool) -> Option<Self> {
        if !modified_files {
            return None;
        }

        let mut executed_results = results
            .iter_mut()
            .filter(|result| result.status.was_executed());
        let Some(result) = executed_results.next() else {
            return Some(Self::PriorityGroup);
        };
        if executed_results.next().is_some() {
            return Some(Self::PriorityGroup);
        }

        result.status = RunStatus::Failed;
        Some(Self::SingleHook)
    }
}

struct PriorityGroupResult {
    results: Vec<HookRunResult>,
    modification: Option<ModificationScope>,
}

impl PriorityGroupResult {
    fn new(mut results: Vec<HookRunResult>, modified_files: bool) -> Self {
        let modification = ModificationScope::from_results(&mut results, modified_files);
        Self {
            results,
            modification,
        }
    }

    fn shows_group_failure(&self, filter: ReportFilter<'_>) -> bool {
        self.modification == Some(ModificationScope::PriorityGroup)
            && filter.shows(RunStatus::Failed)
    }

    fn has_visible_report(&self, filter: ReportFilter<'_>) -> bool {
        self.shows_group_failure(filter)
            || self
                .results
                .iter()
                .any(|result| filter.shows(result.status))
    }

    fn hook_fail_fast(&self) -> bool {
        self.results.iter().any(|result| {
            result.status.was_executed()
                && result.hook.fail_fast
                && (self.modification.is_some() || result.status.is_failure())
        })
    }

    fn failed(&self) -> bool {
        self.modification.is_some() || self.results.iter().any(|result| result.status.is_failure())
    }

    fn should_stop_project(&self, project_fail_fast: bool) -> bool {
        self.failed() && (project_fail_fast || self.hook_fail_fast())
    }
}

#[derive(Clone, Copy)]
struct ReportFilter<'a> {
    hidden_statuses: &'a [HideStatus],
}

impl<'a> ReportFilter<'a> {
    fn new(hidden_statuses: &'a [HideStatus]) -> Self {
        Self { hidden_statuses }
    }

    fn shows(self, status: RunStatus) -> bool {
        let Some(hide_status) = status.hide_status() else {
            return true;
        };
        !self.hidden_statuses.contains(&hide_status)
    }
}

#[expect(clippy::struct_excessive_bools)]
struct HookRunSession<'a> {
    store: &'a Store,
    reporter: HookRunReporter,
    status_printer: StatusPrinter,
    printer: Printer,
    dry_run: bool,
    report_filter: ReportFilter<'a>,
    verbose: bool,
    failed: bool,
    modified_files: bool,
}

impl<'a> HookRunSession<'a> {
    fn new(
        hooks: &[ScheduledHook],
        store: &'a Store,
        dry_run: bool,
        hidden_statuses: &'a [HideStatus],
        verbose: bool,
        show_project_headers: bool,
        printer: Printer,
    ) -> Self {
        let status_printer = StatusPrinter::for_hooks(hooks, printer);
        let reporter =
            HookRunReporter::new(printer, status_printer.bar_len(), show_project_headers);

        Self {
            store,
            reporter,
            status_printer,
            printer,
            dry_run,
            report_filter: ReportFilter::new(hidden_statuses),
            verbose,
            failed: false,
            modified_files: false,
        }
    }

    fn render_project_header(
        &mut self,
        project: &Project,
        failed: bool,
        show_project_headers: bool,
    ) -> Result<()> {
        if !show_project_headers {
            return Ok(());
        }

        self.reporter.suspend(|| {
            writeln!(
                self.status_printer.printer().stdout(),
                "{} {}",
                project_status_marker(failed),
                project.display_name().cyan().bold()
            )
        })?;

        Ok(())
    }

    async fn run_project_level<'project, 'paths>(
        &self,
        project_runs: Vec<ProjectRun<'project>>,
        input: &'paths RunInput,
        file_index: &RunFileIndex<'paths>,
        clean_baseline: bool,
    ) -> Result<Vec<ProjectRunResult<'project>>> {
        let semaphore = Rc::new(Semaphore::new(*HOOK_CONCURRENCY));
        let runs = FuturesUnordered::new();
        for (idx, project_run) in project_runs.into_iter().enumerate() {
            let semaphore = Rc::clone(&semaphore);
            runs.push(async move {
                let project = project_run.project;
                let result = self
                    .run_project(project_run, input, file_index, clean_baseline, semaphore)
                    .await;
                if let Ok(result) = &result {
                    if result.has_visible_report(self.report_filter) {
                        self.reporter.on_project_complete(project, result.failed());
                    } else {
                        self.reporter.hide_project(project);
                    }
                }
                result.map(|result| (idx, result))
            });
        }

        let mut results: Vec<_> = runs.try_collect().await?;
        results.sort_unstable_by_key(|(idx, _)| *idx);
        Ok(results.into_iter().map(|(_, result)| result).collect())
    }

    async fn run_project<'project, 'paths>(
        &self,
        project_run: ProjectRun<'project>,
        input: &'paths RunInput,
        file_index: &RunFileIndex<'paths>,
        clean_baseline: bool,
        semaphore: Rc<Semaphore>,
    ) -> Result<ProjectRunResult<'project>> {
        let project_input = ProjectHookInput::new(input, project_run.project, file_index)?;
        trace!(
            "Files for project `{}` after filtered: {}",
            project_run.project,
            project_input.len()
        );

        // The worktree is only known clean at the start of a depth level. Once
        // an earlier level leaves a diff behind, later projects need a fresh
        // per-project snapshot to avoid attributing that diff to their hooks.
        let mut diff_tracker = if clean_baseline {
            DiffTracker::clean_baseline(project_run.project.path())
        } else {
            DiffTracker::unknown_baseline(project_run.project.path())
        };

        let mut groups = Vec::new();
        let mut stop_after_level = false;

        for group_hooks in project_run.groups {
            let group_requires_diff_tracking = !self.dry_run
                && group_hooks
                    .iter()
                    .filter_map(HookPlan::as_run)
                    .any(|hook| hooks::requires_diff_tracking(hook));
            diff_tracker
                .prepare_for_group(group_requires_diff_tracking)
                .await?;

            let group_results = self
                .run_priority_group(
                    group_hooks,
                    &project_input,
                    file_index.tag_cache(),
                    Rc::clone(&semaphore),
                )
                .await?;

            let known_modified_files = group_results
                .iter()
                .any(|result| result.file_changes == hooks::FileChanges::Modified);
            let needs_diff = !known_modified_files
                && group_results
                    .iter()
                    .any(|result| result.file_changes == hooks::FileChanges::Unknown);
            let diff_detected_modifications = diff_tracker.changed_after_group(needs_diff).await?;
            if known_modified_files {
                // The group is already known to have modified files, so a Git
                // comparison cannot change its result. A later external hook
                // will capture the current worktree before it runs.
                diff_tracker.invalidate();
            }
            let group_modified_files = known_modified_files || diff_detected_modifications;

            let group = PriorityGroupResult::new(group_results, group_modified_files);
            self.update_live_priority_group(&group);
            stop_after_level = group.should_stop_project(project_run.project_fail_fast);
            groups.push(group);

            if stop_after_level {
                break;
            }
        }

        Ok(ProjectRunResult {
            project: project_run.project,
            groups,
            stop_after_level,
        })
    }

    async fn run_priority_group(
        &self,
        group_hooks: Vec<ScheduledHook>,
        project_input: &ProjectHookInput<'_, '_>,
        tag_cache: &FileTagCache,
        semaphore: Rc<Semaphore>,
    ) -> Result<Vec<HookRunResult>> {
        debug!(
            "Running priority group with priority {}: {:?}",
            group_hooks[0].priority,
            group_hooks.iter().map(|hook| &hook.id).collect::<Vec<_>>()
        );

        let runs = FuturesUnordered::new();
        for hook in group_hooks {
            runs.push(run_hook(
                hook,
                project_input,
                tag_cache,
                self.store,
                self.dry_run,
                &self.reporter,
                Rc::clone(&semaphore),
            ));
        }

        runs.try_collect().await
    }

    fn update_live_priority_group(&self, group: &PriorityGroupResult) {
        for result in &group.results {
            let status = result.status;
            match status {
                RunStatus::Passed | RunStatus::Failed if self.report_filter.shows(status) => {
                    self.reporter
                        .on_run_result(&result.hook, status == RunStatus::Passed);
                }
                RunStatus::Passed | RunStatus::Failed => {
                    self.reporter.hide_run_result(&result.hook);
                }
                RunStatus::DryRun | RunStatus::Skipped(_) => {}
            }
        }
    }

    fn finish_project_run(
        &mut self,
        project_result: ProjectRunResult<'_>,
        show_project_headers: bool,
    ) -> Result<bool> {
        let show_project_header =
            show_project_headers && project_result.has_visible_report(self.report_filter);
        self.render_project_header(
            project_result.project,
            project_result.failed(),
            show_project_header,
        )?;
        let hook_prefix = if show_project_header { "  " } else { "" };

        for group in project_result.groups {
            self.finish_priority_group(group, hook_prefix)?;
        }

        Ok(project_result.stop_after_level)
    }

    fn finish_priority_group(
        &mut self,
        mut group: PriorityGroupResult,
        hook_prefix: &str,
    ) -> Result<()> {
        // Print results in a stable order (same order as config within the project).
        group.results.sort_unstable_by_key(|result| result.hook.idx);

        self.failed |= group.failed();
        self.modified_files |= group.modification.is_some();

        self.reporter.clear_completed();
        for result in &group.results {
            if result.shows_details(self.verbose) {
                result.write_log_file()?;
            }
        }
        self.reporter
            .suspend(|| self.render_priority_group(&group, hook_prefix))?;

        Ok(())
    }

    fn render_priority_group(&self, group: &PriorityGroupResult, hook_prefix: &str) -> Result<()> {
        let group_results = &group.results;
        let modifications_belong_to_single_hook =
            group.modification == Some(ModificationScope::SingleHook);
        let show_group_failure = group.shows_group_failure(self.report_filter);

        // Hooks that did not run cannot have modified files, so report them outside
        // the modification group.
        let mut visible_results = group_results
            .iter()
            .filter(|result| self.report_filter.shows(result.status))
            .filter(|result| !show_group_failure || result.status.was_executed())
            .peekable();
        let mut first_result = true;
        let group_output_prefix = if show_group_failure {
            Some(format!("{hook_prefix}{}", "  │ ".dimmed()))
        } else {
            None
        };
        if show_group_failure {
            self.status_printer.write(
                "Files were modified by following hooks",
                hook_prefix,
                RunStatus::Failed,
            )?;
        }

        while let Some(result) = visible_results.next() {
            let status = result.status;

            let connector = if show_group_failure {
                if visible_results.peek().is_none() {
                    "  └ "
                } else if first_result {
                    "  ┌ "
                } else {
                    "  │ "
                }
            } else {
                ""
            };
            first_result = false;
            let prefix = format!("{hook_prefix}{connector}");
            self.status_printer
                .write(&result.hook.name, &prefix, status)?;

            if !status.is_skipped() && result.shows_details(self.verbose) {
                self.render_hook_details(
                    result,
                    hook_prefix,
                    group_output_prefix.as_deref(),
                    modifications_belong_to_single_hook,
                )?;
            }
        }

        if show_group_failure {
            for result in group_results {
                if !result.status.was_executed() && self.report_filter.shows(result.status) {
                    self.status_printer
                        .write(&result.hook.name, hook_prefix, result.status)?;
                }
            }
        }

        Ok(())
    }

    fn render_hook_details(
        &self,
        result: &HookRunResult,
        hook_prefix: &str,
        group_output_prefix: Option<&str>,
        modified_files: bool,
    ) -> Result<()> {
        let detail_prefix = group_output_prefix.unwrap_or(hook_prefix);
        let mut stdout = if result.status.is_failure() {
            self.printer.stdout_important()
        } else {
            self.printer.stdout()
        };

        writeln!(
            stdout,
            "{detail_prefix}{}",
            format!("- hook id: {}", result.hook.id).dimmed()
        )?;
        if !result.hook.alias.is_empty() && result.hook.alias != result.hook.id {
            writeln!(
                stdout,
                "{detail_prefix}{}",
                format!("- hook alias: {}", result.hook.alias).dimmed()
            )?;
        }
        if let Some(description) = result.hook.description.as_deref()
            && let Some(description) = description.trim().lines().next()
        {
            let description = description.trim_end().trim_end_matches('.');
            writeln!(
                stdout,
                "{detail_prefix}{}",
                format!("- description: {description}").dimmed()
            )?;
        }
        if self.verbose || result.hook.verbose {
            writeln!(
                stdout,
                "{detail_prefix}{}",
                format!("- duration: {:.2?}s", result.duration.as_secs_f64()).dimmed()
            )?;
        }
        if result.exit_status != 0 {
            writeln!(
                stdout,
                "{detail_prefix}{}",
                format!("- exit code: {}", result.exit_status).dimmed()
            )?;
        }
        if modified_files {
            writeln!(
                stdout,
                "{detail_prefix}{}",
                "- files were modified by this hook".dimmed()
            )?;
        }

        let output = result.output.trim_ascii();
        if output.is_empty() || result.hook.log_file.is_some() {
            return Ok(());
        }
        let text = sanitize_output(output);
        if text.is_empty() {
            return Ok(());
        }

        let separator = if group_output_prefix.is_some() {
            format!("{hook_prefix}{}", "  │".dimmed())
        } else {
            String::new()
        };
        writeln!(stdout, "{separator}")?;
        for line in text.lines() {
            if line.is_empty() {
                writeln!(stdout, "{separator}")?;
            } else if let Some(group_output_prefix) = group_output_prefix {
                writeln!(stdout, "{group_output_prefix}{line}")?;
            } else {
                writeln!(stdout, "{hook_prefix}  {line}")?;
            }
        }

        Ok(())
    }

    async fn finish(
        &self,
        workspace: &Workspace,
        show_diff_on_failure: bool,
    ) -> Result<ExitStatus> {
        self.reporter.on_complete();

        if self.failed && show_diff_on_failure && self.modified_files {
            if EnvVars::is_under_ci() {
                writeln!(
                    self.printer.stdout(),
                    "{}",
                    indoc::formatdoc! {
                        "\n{}: Some hooks made changes to the files.
                        If you are seeing this message in CI, reproduce locally with: `{}`
                        To run prek as part of Git workflow, use `{}` to set up Git shims.\n",
                        "hint".yellow().bold(),
                        "prek run --all-files".cyan(),
                        "prek install".cyan()
                    }
                )?;
            }

            writeln!(
                self.printer.stdout_important(),
                "All changes made by hooks:"
            )?;

            let color = if *USE_COLOR {
                "--color=always"
            } else {
                "--color=never"
            };
            git::git_cmd()?
                .arg("--no-pager")
                .arg("diff")
                .hidden_args(["--no-ext-diff"])
                .arg(color)
                .arg("--")
                .arg(workspace.root())
                .check(true)
                .spawn()?
                .wait()
                .await?;
        }

        if self.failed {
            Ok(ExitStatus::Failure)
        } else {
            Ok(ExitStatus::Success)
        }
    }
}

struct PriorityGroups {
    hooks: Vec<ScheduledHook>,
}

impl PriorityGroups {
    fn new(hooks: Vec<ScheduledHook>) -> Self {
        Self { hooks }
    }
}

impl Iterator for PriorityGroups {
    type Item = Vec<ScheduledHook>;

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.hooks.first()?;
        let priority = first.priority;
        let next_priority = self
            .hooks
            .iter()
            .position(|hook| hook.priority != priority)
            .unwrap_or(self.hooks.len());

        Some(self.hooks.drain(..next_priority).collect())
    }
}

enum ProjectHookInput<'index, 'paths> {
    Files(&'index ProjectFiles<'paths>),
    MessageFile {
        hook_arg: PathBuf,
        tags: Option<TagSet>,
    },
}

impl<'index, 'paths> ProjectHookInput<'index, 'paths> {
    fn new(
        input: &'paths RunInput,
        project: &Project,
        file_index: &'index RunFileIndex<'paths>,
    ) -> Result<Self> {
        match input {
            RunInput::Files(_) => Ok(Self::Files(file_index.project_files(project))),
            RunInput::MessageFile(path) => {
                let tags = match tags_from_path(path) {
                    Ok(tags) => Some(tags),
                    Err(err) => {
                        error!(filename = ?path.display(), error = %err, "Failed to get tags");
                        None
                    }
                };
                Ok(Self::MessageFile {
                    hook_arg: fs::normalize_path(fs::relative_to(path, project.path())?),
                    tags,
                })
            }
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Files(project_files) => project_files.len(),
            Self::MessageFile { .. } => 1,
        }
    }

    fn run_input_for_hook(&self, hook: &Hook, tag_cache: &FileTagCache) -> HookRunInput<'_> {
        match self {
            Self::Files(project_files) => match hook.pass_filenames {
                // Always-run hooks without filename arguments run regardless of file matches.
                PassFilenames::None if hook.always_run => HookRunInput::without_filenames(true),
                PassFilenames::None => HookRunInput::without_filenames(
                    project_files.has_matching_file(hook, tag_cache),
                ),
                PassFilenames::All | PassFilenames::Limited(_) => {
                    HookRunInput::with_filenames(project_files.matching_filenames(hook, tag_cache))
                }
            },
            Self::MessageFile { hook_arg, .. } => {
                if self.matches_hook(hook, tag_cache) {
                    match hook.pass_filenames {
                        PassFilenames::None => HookRunInput::without_filenames(true),
                        PassFilenames::All | PassFilenames::Limited(_) => {
                            HookRunInput::with_filename(hook_arg)
                        }
                    }
                } else {
                    HookRunInput::without_filenames(false)
                }
            }
        }
    }

    fn matches_hook(&self, hook: &Hook, tag_cache: &FileTagCache) -> bool {
        match self {
            Self::Files(project_files) => project_files.has_matching_file(hook, tag_cache),
            Self::MessageFile { hook_arg, tags } => {
                // `commit-msg` and `prepare-commit-msg` receive Git's special message file,
                // which can live outside a project root, so it bypasses project ownership
                // filtering. Hook-level `files`/`exclude`/`types` filters still apply.
                let hook_filter = HookFileFilter::new(hook);
                hook_filter.matches_filename(hook_arg) && hook_filter.matches_tags(tags.as_ref())
            }
        }
    }
}

enum HookRunInput<'a> {
    Filenames(Vec<&'a Path>),
    Filename(&'a Path),
    WithoutFilenames { matched: bool },
}

impl<'a> HookRunInput<'a> {
    fn with_filenames<I>(filenames: I) -> Self
    where
        I: IntoIterator<Item = &'a Path>,
    {
        Self::Filenames(filenames.into_iter().collect())
    }

    fn with_filename(filename: &'a Path) -> Self {
        Self::Filename(filename)
    }

    fn without_filenames(matched: bool) -> Self {
        Self::WithoutFilenames { matched }
    }

    fn matched(&self) -> bool {
        match self {
            Self::Filenames(filenames) => !filenames.is_empty(),
            Self::Filename(_) => true,
            Self::WithoutFilenames { matched } => *matched,
        }
    }

    fn filename_count(&self) -> usize {
        match self {
            Self::Filenames(filenames) => filenames.len(),
            Self::Filename(_) => 1,
            Self::WithoutFilenames { .. } => 0,
        }
    }

    fn shuffle(&mut self) {
        // Shuffle the files so that they more evenly fill out the xargs
        // partitions, but do it deterministically in case a hook cares about ordering.
        const SEED: u64 = 1_542_676_187;
        if let Self::Filenames(filenames) = self {
            let mut rng = fastrand::Rng::with_seed(SEED);
            rng.shuffle(filenames);
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, clap::ValueEnum)]
pub(crate) enum HideStatus {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum RunStatus {
    Passed,
    Failed,
    DryRun,
    Skipped(SkipReason),
}

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum SkipReason {
    Explicit,
    NoFiles,
}

impl RunStatus {
    fn hide_status(self) -> Option<HideStatus> {
        match self {
            Self::Passed => Some(HideStatus::Passed),
            Self::Failed => Some(HideStatus::Failed),
            Self::Skipped(_) => Some(HideStatus::Skipped),
            Self::DryRun => None,
        }
    }

    fn is_failure(self) -> bool {
        self == Self::Failed
    }

    fn was_executed(self) -> bool {
        matches!(self, Self::Passed | Self::Failed)
    }

    fn is_skipped(self) -> bool {
        matches!(self, Self::Skipped(_))
    }

    fn label(self) -> Styled<&'static str> {
        match self {
            Self::Passed => PASSED,
            Self::Failed => FAILED,
            Self::DryRun => DRY_RUN,
            Self::Skipped(_) => SKIPPED,
        }
    }

    fn suffix(self) -> &'static str {
        match self {
            Self::Skipped(SkipReason::NoFiles) => "(no files to check)",
            Self::Passed | Self::Failed | Self::DryRun | Self::Skipped(SkipReason::Explicit) => "",
        }
    }
}

struct StatusPrinter {
    printer: Printer,
    columns: usize,
}

impl StatusPrinter {
    fn for_hooks<T>(hooks: &[T], printer: Printer) -> Self
    where
        T: Deref<Target = Hook>,
    {
        let name_len = hooks
            .iter()
            .map(|hook| hook.name.width())
            .max()
            .unwrap_or(0);
        let widest_status = RunStatus::Skipped(SkipReason::NoFiles);
        let status_width = widest_status.suffix().width() + widest_status.label().inner().width();
        let columns = std::cmp::max(
            79,
            // Hook name...(no files to check)Skipped
            name_len + 3 + status_width,
        );
        Self { printer, columns }
    }

    fn printer(&self) -> Printer {
        self.printer
    }

    fn bar_len(&self) -> usize {
        self.columns - RunStatus::Passed.label().inner().width()
    }

    fn write(
        &self,
        hook_name: &str,
        prefix: &str,
        status: RunStatus,
    ) -> Result<(), std::fmt::Error> {
        let suffix = status.suffix();
        let status_line = status.label();
        let (prefix, prefix_width) = if prefix.is_empty() {
            (String::new(), 0)
        } else {
            (prefix.dimmed().to_string(), prefix.width())
        };
        let used_width =
            prefix_width + hook_name.width() + suffix.width() + status_line.inner().width();
        let dots = self.columns.saturating_sub(used_width);
        let dots = ".".repeat(dots).green().to_string();
        let line = format!("{prefix}{hook_name}{dots}{suffix}{status_line}");
        if status.is_failure() {
            writeln!(self.printer.stdout_important(), "{line}")
        } else {
            writeln!(self.printer.stdout(), "{line}")
        }
    }
}

struct HookRunResult {
    hook: ScheduledHook,
    status: RunStatus,
    duration: std::time::Duration,
    exit_status: i32,
    output: Vec<u8>,
    file_changes: hooks::FileChanges,
}

impl HookRunResult {
    fn skipped(hook: ScheduledHook) -> Self {
        Self::not_run(hook, SkipReason::Explicit)
    }

    fn no_files(hook: ScheduledHook) -> Self {
        Self::not_run(hook, SkipReason::NoFiles)
    }

    fn not_run(hook: ScheduledHook, reason: SkipReason) -> Self {
        Self {
            hook,
            status: RunStatus::Skipped(reason),
            duration: std::time::Duration::ZERO,
            exit_status: 0,
            output: Vec::new(),
            file_changes: hooks::FileChanges::Unchanged,
        }
    }

    fn shows_details(&self, verbose: bool) -> bool {
        verbose || self.hook.verbose || self.status.is_failure()
    }

    fn write_log_file(&self) -> Result<()> {
        let Some(file) = self.hook.log_file.as_deref() else {
            return Ok(());
        };
        let output = self.output.trim_ascii();
        if output.is_empty() {
            return Ok(());
        }

        let file = Path::new(file);
        let config_file = self.hook.project().config_file();
        let file = if file.is_relative() {
            let config_dir = config_file
                .parent()
                .context("Configuration file must have a parent directory")?;
            config_dir.join(file)
        } else {
            file.to_path_buf()
        };
        let mut file = fs_err::OpenOptions::new()
            .create(true)
            .append(true)
            .open(file)?;
        file.write_all(output)?;
        file.flush()?;

        Ok(())
    }
}

async fn run_hook(
    hook: ScheduledHook,
    project_input: &ProjectHookInput<'_, '_>,
    tag_cache: &FileTagCache,
    store: &Store,
    dry_run: bool,
    reporter: &HookRunReporter,
    semaphore: Rc<Semaphore>,
) -> Result<HookRunResult> {
    let installed_hook = match &hook {
        HookPlan::Run(hook) => hook,
        HookPlan::Skip(_) => return Ok(HookRunResult::skipped(hook)),
    };

    let _permit = if dry_run {
        None
    } else {
        Some(semaphore.acquire(1).await)
    };

    let mut input = project_input.run_input_for_hook(installed_hook, tag_cache);
    let matched = input.matched();
    let filename_count = input.filename_count();
    trace!(
        matched,
        filenames = filename_count,
        "Files for hook `{}` after filtering",
        hook.id,
    );

    if !matched && !installed_hook.always_run {
        return Ok(HookRunResult::no_files(hook));
    }
    let start = std::time::Instant::now();

    let hook_output = if dry_run {
        hooks::HookOutput::unchanged(0, dry_run_hook(installed_hook, &input)?)
    } else {
        input.shuffle();
        match &input {
            HookRunInput::Filenames(filenames) => {
                installed_hook
                    .language
                    .run(store, installed_hook, filenames, reporter)
                    .await
            }
            HookRunInput::Filename(filename) => {
                installed_hook
                    .language
                    .run(store, installed_hook, slice::from_ref(filename), reporter)
                    .await
            }
            HookRunInput::WithoutFilenames { .. } => {
                installed_hook
                    .language
                    .run(store, installed_hook, &[], reporter)
                    .await
            }
        }
        .with_context(|| format!("Failed to run hook `{installed_hook}`"))?
    };
    let hooks::HookOutput {
        exit_status,
        output,
        file_changes,
    } = hook_output;

    let duration = start.elapsed();

    let run_status = if dry_run {
        RunStatus::DryRun
    } else if exit_status == 0 {
        RunStatus::Passed
    } else {
        RunStatus::Failed
    };

    Ok(HookRunResult {
        hook,
        status: run_status,
        duration,
        exit_status,
        output,
        file_changes,
    })
}

fn dry_run_hook(hook: &InstalledHook, input: &HookRunInput<'_>) -> Result<Vec<u8>> {
    let mut output = Vec::new();
    let filename_count = input.filename_count();
    if filename_count != 0 {
        writeln!(output, "`{hook}` would be run on {filename_count} files:")?;
    }

    match input {
        HookRunInput::Filenames(filenames) => {
            for filename in filenames {
                writeln!(output, "- {}", filename.display())?;
            }
        }
        HookRunInput::Filename(filename) => {
            writeln!(output, "- {}", filename.display())?;
        }
        HookRunInput::WithoutFilenames { .. } => {}
    }

    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_printer_write_dots_saturates_instead_of_underflow() {
        let status_printer = StatusPrinter {
            printer: Printer::Silent,
            columns: 10,
        };

        // This would underflow if computed with plain `-` on `usize`.
        let long_name = "this hook name is definitely longer than ten columns";
        status_printer
            .write(long_name, "", RunStatus::Failed)
            .expect("write should not fail");
    }
}
