use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use mea::semaphore::Semaphore;
use owo_colors::OwoColorize;
use prek_consts::env_vars::EnvVars;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PREK_TOML};
use rand::SeedableRng;
use rand::prelude::{SliceRandom, StdRng};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use tokio::sync::mpsc;
use tracing::{debug, trace};
use unicode_width::UnicodeWidthStr;

use crate::cli::reporter::{HookInitReporter, HookInstallReporter, HookRunReporter};
use crate::cli::run::filter::HookFileFilter;
use crate::cli::run::keeper::WorkTreeKeeper;
use crate::cli::run::{
    CollectOptions, FileTagCache, ProjectFiles, RunInput, Selectors, collect_run_input,
};
use crate::cli::{ExitStatus, RunExtraArgs};
use crate::config::{Language, PassFilenames, Stage};
use crate::fs::CWD;
use crate::git::GIT_ROOT;
use crate::hook::{Hook, InstalledHook};
use crate::printer::Printer;
use crate::run::{CONCURRENCY, USE_COLOR};
use crate::store::Store;
use crate::workspace::{Project, Workspace};
use crate::{fs, git, warn_user};

use super::install::{InstallCache, InstallJob, InstallPartitions, Installer, PartitionInstaller};

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub(crate) async fn run(
    store: &Store,
    config: Option<PathBuf>,
    includes: Vec<String>,
    skips: Vec<String>,
    hook_stage: Option<Stage>,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<String>,
    directories: Vec<String>,
    last_commit: bool,
    show_diff_on_failure: bool,
    fail_fast: Option<bool>,
    dry_run: bool,
    refresh: bool,
    extra_args: RunExtraArgs,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    // Convert `--last-commit` to `HEAD~1..HEAD`
    let (from_ref, to_ref) = if last_commit {
        (Some("HEAD~1".to_string()), Some("HEAD".to_string()))
    } else {
        (from_ref, to_ref)
    };

    // Prevent recursive post-checkout hooks.
    if hook_stage == Some(Stage::PostCheckout)
        && EnvVars::is_set(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT)
    {
        return Ok(ExitStatus::Success);
    }

    // Ensure we are in a git repository.
    LazyLock::force(&GIT_ROOT).as_ref()?;

    let should_stash = !all_files && files.is_empty() && directories.is_empty();

    // Check if we have unresolved merge conflict files and fail fast.
    if should_stash && git::has_unmerged_paths().await? {
        anyhow::bail!("You have unmerged paths. Resolve them before running prek");
    }

    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    let selectors = Selectors::load(&includes, &skips, &workspace_root)?;
    let mut workspace =
        Workspace::discover(store, workspace_root, config, Some(&selectors), refresh)?;

    if should_stash {
        workspace.check_configs_staged().await?;
    }

    let reporter = HookInitReporter::new(printer);
    let hooks = {
        let _lock = store.lock_async().await?;
        store.track_configs(workspace.projects().iter().map(|p| p.config_file()))?;

        workspace
            .init_hooks(store, Some(&reporter))
            .await
            .context("Failed to init hooks")?
    };
    let selected_hooks: Vec<_> = hooks
        .into_iter()
        .filter(|h| selectors.matches_hook(h))
        .map(Arc::new)
        .collect();

    selectors.report_unused();

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

    let (filtered_hooks, hook_stage) = if let Some(hook_stage) = hook_stage {
        let hooks = selected_hooks
            .iter()
            .filter(|h| h.stages.contains(hook_stage))
            .cloned()
            .collect::<Vec<_>>();
        (hooks, hook_stage)
    } else {
        // Try filtering by `pre-commit` stage first.
        let mut hook_stage = Stage::PreCommit;
        let mut hooks = selected_hooks
            .iter()
            .filter(|h| h.stages.contains(Stage::PreCommit))
            .cloned()
            .collect::<Vec<_>>();
        if hooks.is_empty() && selectors.includes_only_hook_targets() {
            // If no hooks found for `pre-commit` stage, try fallback to `manual` stage for hooks specified directly.
            hook_stage = Stage::Manual;
            hooks = selected_hooks
                .iter()
                .filter(|h| h.stages.contains(Stage::Manual))
                .cloned()
                .collect();
        }
        (hooks, hook_stage)
    };

    if filtered_hooks.is_empty() {
        debug!(
            stage = %hook_stage,
            "No hooks found for stage after filtering, exit early"
        );
        return Ok(ExitStatus::Success);
    }

    debug!(
        "Hooks going to run: {:?}",
        filtered_hooks.iter().map(|h| &h.id).collect::<Vec<_>>()
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

    set_env_vars(from_ref.as_ref(), to_ref.as_ref(), &extra_args);

    let input = collect_run_input(
        workspace.root(),
        CollectOptions {
            hook_stage,
            from_ref,
            to_ref,
            all_files,
            files,
            directories,
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

    run_hooks(
        &workspace,
        &filtered_hooks,
        input,
        store,
        show_diff_on_failure,
        fail_fast,
        dry_run,
        verbose,
        printer,
    )
    .await
}

// `pre-commit` sets these environment variables for other git hooks.
fn set_env_vars(from_ref: Option<&String>, to_ref: Option<&String>, args: &RunExtraArgs) {
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

struct StatusPrinter {
    printer: Printer,
    columns: usize,
}

impl StatusPrinter {
    const PASSED: &'static str = "Passed";
    const FAILED: &'static str = "Failed";
    const SKIPPED: &'static str = "Skipped";
    const DRY_RUN: &'static str = "Dry Run";
    const NO_FILES: &'static str = "(no files to check)";
    const UNIMPLEMENTED: &'static str = "(unimplemented yet)";

    fn for_hooks<T>(hooks: &[T], printer: Printer) -> Self
    where
        T: std::ops::Deref<Target = Hook>,
    {
        let name_len = hooks
            .iter()
            .map(|hook| hook.name.width())
            .max()
            .unwrap_or(0);
        let columns = std::cmp::max(
            79,
            // Hook name...(no files to check)Skipped
            name_len + 3 + Self::NO_FILES.len() + Self::SKIPPED.len(),
        );
        Self { printer, columns }
    }

    fn printer(&self) -> Printer {
        self.printer
    }

    fn bar_len(&self) -> usize {
        self.columns - Self::PASSED.len()
    }

    fn write(
        &self,
        hook_name: &str,
        prefix: &str,
        status: RunStatus,
    ) -> Result<(), std::fmt::Error> {
        let (suffix, status_line, status_width) = match status {
            RunStatus::NoFiles => (
                Self::NO_FILES,
                Self::SKIPPED.black().on_cyan().to_string(),
                Self::SKIPPED.width(),
            ),
            RunStatus::Unimplemented => (
                Self::UNIMPLEMENTED,
                Self::SKIPPED.black().on_yellow().to_string(),
                Self::SKIPPED.width(),
            ),
            RunStatus::DryRun => (
                "",
                Self::DRY_RUN.on_yellow().to_string(),
                Self::DRY_RUN.width(),
            ),
            RunStatus::Success => (
                "",
                Self::PASSED.on_green().to_string(),
                Self::PASSED.width(),
            ),
            RunStatus::Failed => ("", Self::FAILED.on_red().to_string(), Self::FAILED.width()),
        };
        let (prefix, prefix_width) = if prefix.is_empty() {
            (String::new(), 0)
        } else {
            (prefix.dimmed().to_string(), prefix.width())
        };
        let used_width = prefix_width + hook_name.width() + suffix.width() + status_width;
        let dots = self.columns.saturating_sub(used_width);
        let line = format!(
            "{prefix}{hook_name}{}{suffix}{status_line}",
            ".".repeat(dots),
        );
        match status {
            RunStatus::Failed => {
                writeln!(self.printer.stdout_important(), "{line}")
            }
            _ => writeln!(self.printer.stdout(), "{line}"),
        }
    }
}

enum ProjectHookInput<'a> {
    Files(ProjectFiles<'a>),
    MessageFile {
        absolute_path: &'a Path,
        hook_arg: PathBuf,
    },
}

impl<'a> ProjectHookInput<'a> {
    fn new(
        input: &'a RunInput,
        project: &Project,
        consumed_files: Option<&mut FxHashSet<&'a Path>>,
    ) -> Result<Self> {
        match input {
            RunInput::Files(files) => Ok(Self::Files(ProjectFiles::for_project(
                files.iter(),
                project,
                consumed_files,
            ))),
            RunInput::MessageFile(path) => Ok(Self::MessageFile {
                absolute_path: path,
                hook_arg: fs::normalize_path(fs::relative_to(path, project.path())?),
            }),
        }
    }

    fn len(&self) -> usize {
        match self {
            Self::Files(project_files) => project_files.len(),
            Self::MessageFile { .. } => 1,
        }
    }

    fn for_hook<'input>(
        &'input self,
        hook: &Hook,
        tag_cache: &mut FileTagCache<'a>,
    ) -> HookRunInput<'input>
    where
        'a: 'input,
    {
        match self {
            Self::Files(project_files) => match hook.pass_filenames {
                PassFilenames::None => HookRunInput::matched_without_filenames(
                    project_files.has_match_for_hook(hook, tag_cache),
                ),
                PassFilenames::All | PassFilenames::Limited(_) => {
                    HookRunInput::with_filenames(project_files.for_hook(hook, tag_cache))
                }
            },
            Self::MessageFile {
                absolute_path,
                hook_arg,
            } => {
                // `commit-msg` and `prepare-commit-msg` receive Git's special message file,
                // which can live outside a project root, so it bypasses project ownership
                // filtering. Hook-level `files`/`exclude`/`types` filters still apply.
                let hook_filter = HookFileFilter::new(hook);
                if hook_filter.matches_filename(hook_arg)
                    && hook_filter.matches_tags(tag_cache.tags(absolute_path))
                {
                    match hook.pass_filenames {
                        PassFilenames::None => HookRunInput::matched_without_filenames(true),
                        PassFilenames::All | PassFilenames::Limited(_) => {
                            HookRunInput::with_filenames(vec![hook_arg.as_path()])
                        }
                    }
                } else {
                    HookRunInput::matched_without_filenames(false)
                }
            }
        }
    }
}

struct HookRunInput<'a> {
    has_matching_files: bool,
    filenames: Vec<&'a Path>,
}

impl<'a> HookRunInput<'a> {
    fn with_filenames(filenames: Vec<&'a Path>) -> Self {
        let has_matching_files = !filenames.is_empty();
        Self {
            has_matching_files,
            filenames,
        }
    }

    fn matched_without_filenames(has_matching_files: bool) -> Self {
        Self {
            has_matching_files,
            filenames: Vec::new(),
        }
    }
}

type PlannedHookRun<'a> = InstallJob<HookRunInput<'a>>;
type ReadyHookRun<'a> = (InstalledHook, HookRunInput<'a>);

struct PriorityGroupPlan<'a> {
    skipped_results: Vec<RunResult>,
    runnable_hooks: Vec<PlannedHookRun<'a>>,
}

/// Run all hooks.
#[allow(clippy::fn_params_excessive_bools)]
async fn run_hooks(
    workspace: &Workspace,
    hooks: &[Arc<Hook>],
    input: RunInput,
    store: &Store,
    show_diff_on_failure: bool,
    fail_fast: Option<bool>,
    dry_run: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    debug_assert!(!hooks.is_empty(), "No hooks to run");

    let status_printer = StatusPrinter::for_hooks(hooks, printer);
    let reporter = HookRunReporter::new_execution(printer, status_printer.bar_len());

    let mut success = true;

    // Group hooks by project to run them in order of their depth in the workspace.
    #[allow(clippy::mutable_key_type)]
    let mut project_to_hooks: FxHashMap<&Project, Vec<Arc<Hook>>> =
        FxHashMap::with_capacity_and_hasher(hooks.len(), FxBuildHasher);
    for hook in hooks {
        project_to_hooks
            .entry(hook.project())
            .or_default()
            .push(hook.clone());
    }

    let projects_len = project_to_hooks.len();
    let mut first = true;
    let mut file_modified = false;
    let mut has_unimplemented = false;

    // Track files that have been consumed by orphan projects.
    let mut consumed_files = FxHashSet::default();
    let mut tag_cache = FileTagCache::default();
    let mut executor = HookRunExecutor::new(store, dry_run, &reporter);

    'outer: for project in workspace.all_projects() {
        let project_input = ProjectHookInput::new(&input, project, Some(&mut consumed_files))?;

        let Some(mut hooks) = project_to_hooks.remove(project) else {
            continue;
        };
        trace!(
            "Files for project `{project}` after filtered: {}",
            project_input.len()
        );

        // Sort hooks by priority (lower number means higher priority).
        // If two hooks have the same priority, preserve their original order from the config.
        hooks.sort_by(|a, b| a.priority.cmp(&b.priority).then(a.idx.cmp(&b.idx)));

        if projects_len > 1 || !project.is_root() {
            reporter.suspend(|| {
                writeln!(
                    status_printer.printer().stdout(),
                    "{}{}",
                    if first { "" } else { "\n" },
                    format!("Running hooks for `{}`:", project.to_string().cyan()).bold()
                )
            })?;
            first = false;
        }
        let mut prev_diff = git::get_diff(project.path()).await?;

        let project_fail_fast = fail_fast.or(project.config().fail_fast).unwrap_or(false);

        for group_hooks in PriorityGroups::new(hooks) {
            let PriorityGroupPlan {
                skipped_results: mut group_results,
                runnable_hooks,
            } = plan_priority_group(group_hooks, &project_input, &mut tag_cache);

            if !runnable_hooks.is_empty() {
                let mut run_results = executor.run(runnable_hooks).await?;
                group_results.append(&mut run_results);
            }

            // Print results in a stable order (same order as config within the project).
            group_results.sort_unstable_by_key(|a| a.hook.idx);

            // Check if any files were modified by this group of hooks.
            let all_skipped = group_results.iter().all(|r| r.status.is_skipped());
            let group_modified_files = if !all_skipped {
                let curr_diff = git::get_diff(project.path()).await?;
                let group_modified_files = curr_diff != prev_diff;
                prev_diff = curr_diff;
                group_modified_files
            } else {
                false
            };

            if group_modified_files {
                file_modified = true;
            }

            reporter.clear_completed();
            reporter.suspend(|| {
                render_priority_group(
                    printer,
                    &status_printer,
                    &group_results,
                    verbose,
                    group_modified_files,
                )
            })?;

            let hook_fail_fast = apply_group_outcome(
                &group_results,
                group_modified_files,
                &mut success,
                &mut has_unimplemented,
            );

            if !success && (project_fail_fast || hook_fail_fast) {
                break 'outer;
            }
        }
    }

    reporter.on_complete();

    if has_unimplemented {
        warn_user!(
            "Some hooks were skipped because their languages are unimplemented.\nWe're working hard to support more languages. Check out current support status at {}.",
            "https://prek.j178.dev/languages/".cyan().underline()
        );
    }

    if !success && show_diff_on_failure && file_modified {
        if EnvVars::is_under_ci() {
            writeln!(
                printer.stdout(),
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

        writeln!(printer.stdout_important(), "All changes made by hooks:")?;

        let color = if *USE_COLOR {
            "--color=always"
        } else {
            "--color=never"
        };
        git::git_cmd("git diff")?
            .arg("--no-pager")
            .arg("diff")
            .arg("--no-ext-diff")
            .arg(color)
            .arg("--")
            .arg(workspace.root())
            .check(true)
            .spawn()?
            .wait()
            .await?;
    }

    if success {
        Ok(ExitStatus::Success)
    } else {
        Ok(ExitStatus::Failure)
    }
}

struct PriorityGroups {
    hooks: Vec<Arc<Hook>>,
    idx: usize,
}

impl PriorityGroups {
    fn new(hooks: Vec<Arc<Hook>>) -> Self {
        Self { hooks, idx: 0 }
    }
}

impl Iterator for PriorityGroups {
    type Item = Vec<Arc<Hook>>;

    fn next(&mut self) -> Option<Self::Item> {
        let first = self.hooks.get(self.idx)?;
        let priority = first.priority;
        let start = self.idx;

        while self
            .hooks
            .get(self.idx)
            .is_some_and(|hook| hook.priority == priority)
        {
            self.idx += 1;
        }

        Some(self.hooks[start..self.idx].to_vec())
    }
}

fn plan_priority_group<'input, 'paths>(
    group_hooks: Vec<Arc<Hook>>,
    input: &'input ProjectHookInput<'paths>,
    tag_cache: &mut FileTagCache<'paths>,
) -> PriorityGroupPlan<'input>
where
    'paths: 'input,
{
    debug!(
        "Running priority group with priority {} with concurrency {}: {:?}",
        group_hooks[0].priority,
        *CONCURRENCY,
        group_hooks.iter().map(|h| &h.id).collect::<Vec<_>>()
    );

    let mut skipped_results = Vec::new();
    let mut runnable_hooks = Vec::new();

    for hook in group_hooks {
        let hook_input = input.for_hook(&hook, tag_cache);
        trace!(
            matched = hook_input.has_matching_files,
            filenames = hook_input.filenames.len(),
            "Files for hook `{}` after filtering",
            hook.id,
        );

        if !hook_input.has_matching_files && !hook.always_run {
            skipped_results.push(RunResult::from_status(
                InstalledHook::NoNeedInstall(hook),
                RunStatus::NoFiles,
            ));
            continue;
        }

        if !Language::supported(hook.language) {
            skipped_results.push(RunResult::from_status(
                InstalledHook::NoNeedInstall(hook),
                RunStatus::Unimplemented,
            ));
            continue;
        }

        runnable_hooks.push(InstallJob::new(hook, hook_input));
    }

    PriorityGroupPlan {
        skipped_results,
        runnable_hooks,
    }
}

struct HookRunExecutor<'a> {
    store: &'a Store,
    install_cache: Option<InstallCache>,
    dry_run: bool,
    reporter: &'a HookRunReporter,
}

impl<'a> HookRunExecutor<'a> {
    fn new(store: &'a Store, dry_run: bool, reporter: &'a HookRunReporter) -> Self {
        Self {
            store,
            install_cache: None,
            dry_run,
            reporter,
        }
    }

    async fn run(&mut self, hooks: Vec<PlannedHookRun<'_>>) -> Result<Vec<RunResult>> {
        if hooks.is_empty() {
            return Ok(Vec::new());
        }

        let install_reporter = HookInstallReporter::from_run(self.reporter);
        let pipeline = HookRunPipeline::new(self.store, self.reporter, self.dry_run);
        // Install holds the store lock. The ready queue must not block on run concurrency, or a
        // hook that recursively invokes prek could wait on the same lock while install is stalled.
        let (ready_tx, ready_rx) = mpsc::unbounded_channel();

        let install_future = async {
            let _lock = self.store.lock_async().await?;
            let installer = Installer::for_jobs(
                self.store,
                &install_reporter,
                &mut self.install_cache,
                &hooks,
            )
            .await;
            pipeline
                .install_ready_hooks(installer, hooks, ready_tx)
                .await
        };
        let run_future = pipeline.run_ready_hooks(ready_rx);

        let (install_result, run_result) = futures::future::join(install_future, run_future).await;
        let installed_hooks = install_result?;
        let results = run_result?;

        if let Some(cache) = &mut self.install_cache {
            cache.add_installed(&installed_hooks);
        }

        Ok(results)
    }
}

#[derive(Clone)]
struct HookRunPipeline<'a> {
    store: &'a Store,
    /// Run concurrency is independent from install concurrency.
    run_semaphore: Rc<Semaphore>,
    reporter: &'a HookRunReporter,
    dry_run: bool,
}

impl<'a> HookRunPipeline<'a> {
    fn new(store: &'a Store, reporter: &'a HookRunReporter, dry_run: bool) -> Self {
        Self {
            store,
            run_semaphore: Rc::new(Semaphore::new(*CONCURRENCY)),
            reporter,
            dry_run,
        }
    }

    async fn install_ready_hooks<'input>(
        &self,
        installer: Installer<'a>,
        hooks: Vec<PlannedHookRun<'input>>,
        ready_tx: mpsc::UnboundedSender<ReadyHookRun<'input>>,
    ) -> Result<Vec<InstalledHook>> {
        let mut installed_hooks = Vec::new();
        let mut futures = FuturesUnordered::new();

        for partition in InstallPartitions::new(hooks) {
            let mut partition_installer = installer.partition();
            let ready_tx = ready_tx.clone();
            let store = self.store;
            futures.push(async move {
                Self::install_partition(store, &mut partition_installer, partition, ready_tx).await
            });
        }

        while let Some(result) = futures.next().await {
            let mut partition_installed_hooks = result?;
            installed_hooks.append(&mut partition_installed_hooks);
        }

        Ok(installed_hooks)
    }

    async fn install_partition<'input>(
        store: &Store,
        installer: &mut PartitionInstaller<'a>,
        hooks: Vec<PlannedHookRun<'input>>,
        ready_tx: mpsc::UnboundedSender<ReadyHookRun<'input>>,
    ) -> Result<Vec<InstalledHook>> {
        let mut installed_hooks = Vec::with_capacity(hooks.len());

        for hook in hooks {
            let (installed_hook, input) = installer.install_job(store, hook).await?;
            installed_hooks.push(installed_hook.clone());

            if ready_tx.send((installed_hook, input)).is_err() {
                break;
            }
        }

        Ok(installed_hooks)
    }

    async fn run_ready_hooks(
        &self,
        mut ready_rx: mpsc::UnboundedReceiver<ReadyHookRun<'_>>,
    ) -> Result<Vec<RunResult>> {
        let mut results = Vec::new();
        let mut runs = FuturesUnordered::new();
        let mut ready_open = true;

        loop {
            tokio::select! {
                ready = ready_rx.recv(), if ready_open => {
                    match ready {
                        Some((installed_hook, input)) => {
                            let run_semaphore = Rc::clone(&self.run_semaphore);
                            let store = self.store;
                            let reporter = self.reporter;
                            let dry_run = self.dry_run;

                            runs.push(async move {
                                let _permit = run_semaphore.acquire(1).await;
                                run_hook(installed_hook, input, store, dry_run, reporter).await
                            });
                        }
                        None => ready_open = false,
                    }
                }
                result = runs.next(), if !runs.is_empty() => {
                    if let Some(result) = result {
                        results.push(result?);
                    }
                }
                else => break,
            }
        }

        Ok(results)
    }
}

fn render_priority_group(
    printer: Printer,
    status_printer: &StatusPrinter,
    group_results: &[RunResult],
    verbose: bool,
    group_modified_files: bool,
) -> Result<()> {
    // Only show a special group UI when the group failed due to file modifications.
    // Hooks in a priority group run in parallel, so we can't attribute modifications to a single hook.
    let show_group_ui = group_modified_files && group_results.len() > 1;
    let single_hook_modified_files = group_results.len() == 1 && group_modified_files;
    let group_prefix = if show_group_ui {
        format!("{}", "  │ ".dimmed())
    } else {
        String::new()
    };

    if show_group_ui {
        status_printer.write(
            "Files were modified by following hooks",
            "",
            RunStatus::Failed,
        )?;
    }

    for (i, result) in group_results.iter().enumerate() {
        let prefix = if show_group_ui {
            if i == 0 {
                "  ┌ "
            } else if i + 1 == group_results.len() {
                "  └ "
            } else {
                "  │ "
            }
        } else {
            ""
        };

        // If a single hook modified files, treat it as failed.
        let status = if single_hook_modified_files && result.status == RunStatus::Success {
            RunStatus::Failed
        } else {
            result.status
        };

        status_printer.write(&result.hook.name, prefix, status)?;

        if matches!(status, RunStatus::NoFiles | RunStatus::Unimplemented) {
            continue;
        }

        let mut stdout = match status {
            RunStatus::Failed => printer.stdout_important(),
            _ => printer.stdout(),
        };

        if verbose || result.hook.verbose || status == RunStatus::Failed {
            writeln!(
                stdout,
                "{group_prefix}{}",
                format!("- hook id: {}", result.hook.id).dimmed()
            )?;
            if verbose || result.hook.verbose {
                writeln!(
                    stdout,
                    "{group_prefix}{}",
                    format!("- duration: {:.2?}s", result.duration.as_secs_f64()).dimmed()
                )?;
            }
            if result.exit_status != 0 {
                writeln!(
                    stdout,
                    "{group_prefix}{}",
                    format!("- exit code: {}", result.exit_status).dimmed()
                )?;
            }
            if single_hook_modified_files {
                writeln!(
                    stdout,
                    "{group_prefix}{}",
                    "- files were modified by this hook".dimmed()
                )?;
            }

            let output = result.output.trim_ascii();
            if !output.is_empty() {
                if let Some(file) = result.hook.log_file.as_deref() {
                    let mut file = fs_err::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(file)?;
                    file.write_all(output)?;
                    file.flush()?;
                } else {
                    if show_group_ui {
                        writeln!(stdout, "{}", "  │".dimmed())?;
                    } else {
                        writeln!(stdout)?;
                    }
                    let text = String::from_utf8_lossy(output);
                    for line in text.lines() {
                        if line.is_empty() {
                            if show_group_ui {
                                writeln!(stdout, "{}", "  │".dimmed())?;
                            } else {
                                writeln!(stdout)?;
                            }
                        } else {
                            if show_group_ui {
                                writeln!(stdout, "{group_prefix}{line}")?;
                            } else {
                                writeln!(stdout, "  {line}")?;
                            }
                        }
                    }
                }
            }
        }
    }

    Ok(())
}

fn apply_group_outcome(
    group_results: &[RunResult],
    group_modified_files: bool,
    success: &mut bool,
    has_unimplemented: &mut bool,
) -> bool {
    let mut hook_fail_fast = false;

    for RunResult { hook, status, .. } in group_results {
        *has_unimplemented |= status.is_unimplemented();

        let ok = if group_modified_files {
            false
        } else {
            status.as_bool()
        };
        *success &= ok;

        if !ok && hook.fail_fast {
            hook_fail_fast = true;
        }
    }

    hook_fail_fast
}

/// Shuffle the files so that they more evenly fill out the xargs
/// partitions, but do it deterministically in case a hook cares about ordering.
fn shuffle<T>(filenames: &mut [T]) {
    const SEED: u64 = 1_542_676_187;
    let mut rng = StdRng::seed_from_u64(SEED);
    filenames.shuffle(&mut rng);
}

#[derive(Copy, Clone, Eq, PartialEq)]
enum RunStatus {
    Success,
    Failed,
    DryRun,
    NoFiles,
    Unimplemented,
}

impl RunStatus {
    fn as_bool(self) -> bool {
        matches!(
            self,
            Self::Success | Self::NoFiles | Self::DryRun | Self::Unimplemented
        )
    }

    fn is_unimplemented(self) -> bool {
        matches!(self, Self::Unimplemented)
    }

    fn is_skipped(self) -> bool {
        matches!(self, Self::DryRun | Self::NoFiles | Self::Unimplemented)
    }
}

struct RunResult {
    hook: InstalledHook,
    status: RunStatus,
    duration: std::time::Duration,
    exit_status: i32,
    output: Vec<u8>,
}

impl RunResult {
    fn from_status(hook: InstalledHook, status: RunStatus) -> Self {
        Self {
            hook,
            status,
            duration: std::time::Duration::ZERO,
            exit_status: 0,
            output: Vec::new(),
        }
    }
}

async fn run_hook(
    hook: InstalledHook,
    mut input: HookRunInput<'_>,
    store: &Store,
    dry_run: bool,
    reporter: &HookRunReporter,
) -> Result<RunResult> {
    if !input.has_matching_files && !hook.always_run {
        return Ok(RunResult::from_status(hook, RunStatus::NoFiles));
    }
    if !Language::supported(hook.language) {
        return Ok(RunResult::from_status(hook, RunStatus::Unimplemented));
    }
    let start = std::time::Instant::now();

    let filenames = match hook.pass_filenames {
        PassFilenames::All | PassFilenames::Limited(_) => {
            shuffle(&mut input.filenames);
            input.filenames
        }
        PassFilenames::None => vec![],
    };

    let (exit_status, hook_output) = if dry_run {
        let mut output = Vec::new();
        if !filenames.is_empty() {
            writeln!(
                output,
                "`{}` would be run on {} files:",
                hook,
                filenames.len()
            )?;
        }
        for filename in filenames {
            writeln!(output, "- {}", filename.display())?;
        }
        (0, output)
    } else {
        hook.language
            .run(&hook, &filenames, store, reporter)
            .await
            .with_context(|| format!("Failed to run hook `{hook}`"))?
    };

    let duration = start.elapsed();

    let run_status = if dry_run {
        RunStatus::DryRun
    } else if exit_status == 0 {
        RunStatus::Success
    } else {
        RunStatus::Failed
    };

    Ok(RunResult {
        hook,
        status: run_status,
        duration,
        exit_status,
        output: hook_output,
    })
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
