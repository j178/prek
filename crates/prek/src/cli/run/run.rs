use std::fmt::Write as _;
use std::io::Write as _;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use mea::semaphore::Semaphore;
use owo_colors::OwoColorize;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PREK_TOML};
use prek_identify::{TagSet, tags_from_path};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use tracing::{debug, error, trace};
use unicode_width::UnicodeWidthStr;

use crate::cli::reporter::{HookInitReporter, HookInstallReporter};
use crate::cli::run::diff::DiffTracker;
use crate::cli::run::filter::{RunInputMode, stage_uses_message_file_input};
use crate::cli::run::install::{InstallCache, install_hooks};
use crate::cli::run::keeper::WorkTreeKeeper;
use crate::cli::run::{
    CollectOptions, FileTagCache, GroupFilters, HookFileFilter, HookRunReporter, ProjectFiles,
    RunInput, Selectors, collect_run_input, project_status_marker,
};
use crate::cli::{ExitStatus, RunExtraArgs};
use crate::config::{PassFilenames, Stage};
use crate::fs::CWD;
use crate::git::GIT_ROOT;
use crate::hook::{Hook, InstalledHook};
use crate::printer::Printer;
use crate::run::{HOOK_CONCURRENCY, USE_COLOR};
use crate::store::Store;
use crate::workspace::{HookInitFilters, Project, Workspace};
use crate::{fs, git, hooks, warn_user};

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub(crate) async fn run(
    store: &Store,
    config: Option<PathBuf>,
    includes: Vec<String>,
    skips: Vec<String>,
    groups: Vec<String>,
    no_groups: Vec<String>,
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
        && EnvVars.is_set(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT)
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
    let group_filters = GroupFilters::parse(&groups, &no_groups)?;
    let has_group_filters = group_filters.has_filters();
    let workspace = Workspace::discover(store, workspace_root, config, Some(&selectors), refresh)?;

    if should_stash {
        workspace.check_configs_staged().await?;
    }

    let reporter = HookInitReporter::new(printer);
    let hooks = {
        let _lock = store.lock_async().await?;
        store.track_configs(
            workspace
                .projects()
                .iter()
                .map(|project| project.config_file()),
        )?;

        workspace
            .init_hooks(
                store,
                HookInitFilters::new(Some(&selectors), Some(&group_filters)),
                Some(&reporter),
            )
            .await
            .context("Failed to init hooks")?
    };
    let selected_hooks: Vec<_> = hooks
        .into_iter()
        .filter(|h| selectors.matches_hook(h))
        .filter(|h| group_filters.matches_hook(h))
        .map(Arc::new)
        .collect();

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
    let filtered_hooks: Vec<Arc<Hook>> = if let Some(stage_filter) = stage_filter {
        selected_hooks
            .iter()
            .filter(|h| h.stages.contains(stage_filter))
            .cloned()
            .collect()
    } else {
        // Group selection without an explicit stage uses normal file input, so
        // hooks that can only consume Git message files cannot run correctly.
        selected_hooks
            .into_iter()
            .filter(|hook| !uses_only_message_file_input(hook))
            .collect()
    };

    if filtered_hooks.is_empty() {
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
            input_mode,
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

    let tag_cache = if let RunInput::Files(files) = &input {
        FileTagCache::from_paths(files.iter().map(PathBuf::as_path))
    } else {
        FileTagCache::default()
    };
    let installed_hooks = ensure_hooks_installed(
        store,
        printer,
        &workspace,
        &input,
        &tag_cache,
        &filtered_hooks,
    )
    .await?;

    run_hooks(
        &workspace,
        &input,
        &tag_cache,
        &installed_hooks,
        store,
        show_diff_on_failure,
        fail_fast,
        dry_run,
        should_stash,
        verbose,
        printer,
    )
    .await
}

fn infer_stage_and_input_mode(
    explicit_stage: Option<Stage>,
    has_group_filters: bool,
    selected_hooks: &[Arc<Hook>],
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
    let stage = if selectors.includes_only_hook_targets()
        && !selected_hooks
            .iter()
            .any(|hook| hook.stages.contains(Stage::PreCommit))
    {
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

/// Ensure installable hooks have environments and return the form expected by the runner.
///
/// Hooks that do not need an environment are returned as-is. Hooks that need an
/// environment first try the install cache; only cache misses are filtered
/// against the run input before installation.
async fn ensure_hooks_installed<'paths>(
    store: &Store,
    printer: Printer,
    workspace: &Workspace,
    input: &'paths RunInput,
    tag_cache: &FileTagCache<'paths>,
    hooks: &[Arc<Hook>],
) -> Result<Vec<InstalledHook>> {
    let env_hooks = hooks
        .iter()
        .filter(|hook| hook.needs_install_env())
        .cloned()
        .collect::<Vec<_>>();

    if env_hooks.is_empty() {
        return Ok(hooks
            .iter()
            .map(|hook| InstalledHook::NoNeedInstall(hook.clone()))
            .collect());
    }

    let _lock = store.lock_async().await?;
    let mut install_cache = InstallCache::new();
    let mut installed_by_hook = FxHashMap::default();
    let mut missing_env_hooks = Vec::new();

    // Resolve the cache before file filtering so already-installed hooks keep their exact
    // environment, while missing hooks still avoid install when they would not run.
    for hook in env_hooks {
        if let Some(installed_hook) = install_cache.installed_hook(store, hook.clone()).await {
            installed_by_hook.insert(hook_key(&hook), installed_hook);
        } else {
            missing_env_hooks.push(hook.clone());
        }
    }

    let hooks_to_install =
        select_hooks_to_install(workspace, input, tag_cache, &missing_env_hooks)?;
    if !hooks_to_install.is_empty() {
        let reporter = HookInstallReporter::new(printer);
        let installed_hooks =
            install_hooks(hooks_to_install, store, &reporter, &mut install_cache).await?;
        reporter.on_complete();

        for installed_hook in installed_hooks {
            installed_by_hook.insert(hook_key(&installed_hook), installed_hook);
        }
    }

    Ok(hooks
        .iter()
        .map(|hook| {
            installed_by_hook
                .remove(&hook_key(hook))
                .unwrap_or_else(|| InstalledHook::NoNeedInstall(hook.clone()))
        })
        .collect())
}

/// Return the missing environment hooks that should actually be installed.
///
/// The input hooks are already known to need an environment and be missing from
/// the install cache. This applies language support and run-input filtering so
/// hooks that would not run do not get installed.
fn select_hooks_to_install<'paths>(
    workspace: &Workspace,
    input: &'paths RunInput,
    tag_cache: &FileTagCache<'paths>,
    hooks: &[Arc<Hook>],
) -> Result<Vec<Arc<Hook>>> {
    #[allow(clippy::mutable_key_type)]
    let mut project_to_hooks: FxHashMap<&Project, Vec<Arc<Hook>>> =
        FxHashMap::with_capacity_and_hasher(workspace.all_projects().len(), FxBuildHasher);
    for hook in hooks {
        project_to_hooks
            .entry(hook.project())
            .or_default()
            .push(hook.clone());
    }

    let mut hooks_to_install = Vec::new();
    let mut consumed_files = FxHashSet::default();

    for project in workspace.all_projects() {
        match input {
            RunInput::Files(files) => {
                let mut project_consumed_files = FxHashSet::default();
                let Some(hooks) = project_to_hooks.remove(project.as_ref()) else {
                    ProjectFiles::consume_for_project(
                        files.iter(),
                        project,
                        Some(&consumed_files),
                        &mut project_consumed_files,
                    );
                    consumed_files.extend(project_consumed_files);
                    continue;
                };

                let mut candidates = hooks
                    .into_iter()
                    .map(|hook| {
                        let matches = hook.always_run;
                        (hook, matches)
                    })
                    .collect::<Vec<_>>();
                let mut remaining = candidates.iter().filter(|(_, matches)| !*matches).count();

                ProjectFiles::visit_for_project(
                    files.iter(),
                    project,
                    Some(&consumed_files),
                    Some(&mut project_consumed_files),
                    |project_file| {
                        for (hook, matches) in &mut candidates {
                            if *matches {
                                continue;
                            }

                            let hook_filter = HookFileFilter::new(hook);
                            if hook_filter.matches_project_file(&project_file, tag_cache) {
                                *matches = true;
                                remaining -= 1;
                            }
                        }

                        if remaining == 0 {
                            return ControlFlow::Break(());
                        }

                        ControlFlow::Continue(())
                    },
                );
                consumed_files.extend(project_consumed_files);

                for (hook, matches) in candidates {
                    if matches {
                        hooks_to_install.push(hook);
                    }
                }
            }
            RunInput::MessageFile(_) => {
                let Some(hooks) = project_to_hooks.remove(project.as_ref()) else {
                    continue;
                };

                let project_input = ProjectHookInput::new(input, project, None, None)?;
                for hook in hooks {
                    if hook.always_run || project_input.matches_hook(&hook, tag_cache) {
                        hooks_to_install.push(hook);
                    }
                }
            }
        }
    }

    Ok(hooks_to_install)
}

fn hook_key(hook: &Hook) -> (usize, usize) {
    // Hook indexes are scoped to a project config, so workspace runs need the project index too.
    (hook.project().idx(), hook.idx)
}

#[allow(clippy::fn_params_excessive_bools)]
async fn run_hooks<'paths>(
    workspace: &Workspace,
    input: &'paths RunInput,
    tag_cache: &FileTagCache<'paths>,
    hooks: &[InstalledHook],
    store: &Store,
    show_diff_on_failure: bool,
    fail_fast: Option<bool>,
    dry_run: bool,
    worktree_cleaned: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    debug_assert!(!hooks.is_empty(), "No hooks to run");

    // Group hooks by project to run them in order of their depth in the workspace.
    #[allow(clippy::mutable_key_type)]
    let mut project_to_hooks: FxHashMap<&Project, Vec<InstalledHook>> =
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
        verbose,
        show_project_headers,
        printer,
    );
    let mut consumed_files = FxHashSet::default();

    for projects in ProjectDepthGroups::new(workspace.all_projects()) {
        let clean_baseline = worktree_cleaned && !session.file_modified;
        let mut level_consumed_files = FxHashSet::default();
        let mut project_runs = Vec::new();

        for project in projects {
            let Some(mut hooks) = project_to_hooks.remove(project.as_ref()) else {
                if let RunInput::Files(files) = input {
                    ProjectFiles::consume_for_project(
                        files.iter(),
                        project,
                        Some(&consumed_files),
                        &mut level_consumed_files,
                    );
                }
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

        let project_results = session
            .run_project_level(
                project_runs,
                input,
                &consumed_files,
                tag_cache,
                clean_baseline,
            )
            .await?;
        let mut stop_after_level = false;

        for project_result in project_results {
            level_consumed_files.extend(project_result.consumed_files.iter().copied());
            stop_after_level |= session.finish_project_run(project_result, show_project_headers)?;
        }

        consumed_files.extend(level_consumed_files);

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
    groups: Vec<Vec<InstalledHook>>,
}

struct ProjectRunResult<'project, 'paths> {
    project: &'project Project,
    groups: Vec<ProjectGroupRunResult>,
    consumed_files: FxHashSet<&'paths Path>,
    stop_after_level: bool,
}

impl ProjectRunResult<'_, '_> {
    fn failed(&self) -> bool {
        self.groups.iter().any(ProjectGroupRunResult::failed)
    }
}

struct ProjectGroupRunResult {
    results: Vec<RunResult>,
    modified_files: bool,
}

impl ProjectGroupRunResult {
    fn hook_fail_fast(&self) -> bool {
        self.results.iter().any(|result| {
            let ok = if self.modified_files {
                false
            } else {
                result.status.as_bool()
            };
            !ok && result.hook.fail_fast
        })
    }

    fn failed(&self) -> bool {
        self.modified_files || self.results.iter().any(|result| !result.status.as_bool())
    }

    fn should_stop_project(&self, project_fail_fast: bool) -> bool {
        self.failed() && (project_fail_fast || self.hook_fail_fast())
    }
}

#[allow(clippy::struct_excessive_bools)]
struct HookRunSession<'a> {
    store: &'a Store,
    reporter: HookRunReporter,
    status_printer: StatusPrinter,
    printer: Printer,
    dry_run: bool,
    verbose: bool,
    success: bool,
    file_modified: bool,
}

impl<'a> HookRunSession<'a> {
    fn new(
        hooks: &[InstalledHook],
        store: &'a Store,
        dry_run: bool,
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
            verbose,
            success: true,
            file_modified: false,
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
        consumed_files: &FxHashSet<&'paths Path>,
        tag_cache: &FileTagCache<'paths>,
        clean_baseline: bool,
    ) -> Result<Vec<ProjectRunResult<'project, 'paths>>> {
        let semaphore = Rc::new(Semaphore::new(*HOOK_CONCURRENCY));
        let mut runs = FuturesUnordered::new();
        for (idx, project_run) in project_runs.into_iter().enumerate() {
            let semaphore = Rc::clone(&semaphore);
            runs.push(async move {
                let project = project_run.project;
                let result = self
                    .run_project(
                        project_run,
                        input,
                        consumed_files,
                        tag_cache,
                        clean_baseline,
                        semaphore,
                    )
                    .await;
                if let Ok(result) = &result {
                    self.reporter.on_project_complete(project, result.failed());
                }
                result.map(|result| (idx, result))
            });
        }

        let mut results = Vec::new();
        while let Some(result) = runs.next().await {
            results.push(result?);
        }

        results.sort_unstable_by_key(|(idx, _)| *idx);
        Ok(results.into_iter().map(|(_, result)| result).collect())
    }

    async fn run_project<'project, 'paths>(
        &self,
        project_run: ProjectRun<'project>,
        input: &'paths RunInput,
        consumed_files: &FxHashSet<&'paths Path>,
        tag_cache: &FileTagCache<'paths>,
        clean_baseline: bool,
        semaphore: Rc<Semaphore>,
    ) -> Result<ProjectRunResult<'project, 'paths>> {
        let mut project_consumed_files = FxHashSet::default();
        let project_input = ProjectHookInput::new(
            input,
            project_run.project,
            Some(consumed_files),
            Some(&mut project_consumed_files),
        )?;
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
            let group_may_modify_files =
                !self.dry_run && group_hooks.iter().any(|hook| hooks::may_modify_files(hook));
            diff_tracker
                .prepare_for_group(group_may_modify_files)
                .await?;

            let group_results = self
                .run_priority_group(
                    group_hooks,
                    &project_input,
                    tag_cache,
                    Rc::clone(&semaphore),
                )
                .await?;
            let all_skipped = group_results
                .iter()
                .all(|result| result.status.is_skipped());
            let group_modified_files = diff_tracker
                .changed_after_group(group_may_modify_files, all_skipped)
                .await?;

            let group = ProjectGroupRunResult {
                results: group_results,
                modified_files: group_modified_files,
            };
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
            consumed_files: project_consumed_files,
            stop_after_level,
        })
    }

    async fn run_priority_group(
        &self,
        group_hooks: Vec<InstalledHook>,
        project_input: &ProjectHookInput<'_>,
        tag_cache: &FileTagCache<'_>,
        semaphore: Rc<Semaphore>,
    ) -> Result<Vec<RunResult>> {
        debug!(
            "Running priority group with priority {}: {:?}",
            group_hooks[0].priority,
            group_hooks.iter().map(|hook| &hook.id).collect::<Vec<_>>()
        );

        let mut runs = FuturesUnordered::new();
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

        let mut group_results = Vec::new();
        while let Some(result) = runs.next().await {
            group_results.push(result?);
        }
        Ok(group_results)
    }

    fn update_live_priority_group(&self, group: &ProjectGroupRunResult) {
        let single_hook_modified_files = group.results.len() == 1 && group.modified_files;

        for result in &group.results {
            let status = if single_hook_modified_files && result.status == RunStatus::Success {
                RunStatus::Failed
            } else {
                result.status
            };

            if !status.is_skipped() {
                self.reporter.on_run_result(&result.hook, status.as_bool());
            }
        }
    }

    fn finish_project_run(
        &mut self,
        project_result: ProjectRunResult<'_, '_>,
        show_project_headers: bool,
    ) -> Result<bool> {
        self.render_project_header(
            project_result.project,
            project_result.failed(),
            show_project_headers,
        )?;
        let hook_prefix = if show_project_headers { "  " } else { "" };

        for group in project_result.groups {
            self.finish_priority_group(group, hook_prefix)?;
        }

        Ok(project_result.stop_after_level)
    }

    fn finish_priority_group(
        &mut self,
        group: ProjectGroupRunResult,
        hook_prefix: &str,
    ) -> Result<()> {
        let ProjectGroupRunResult {
            mut results,
            modified_files,
        } = group;
        // Print results in a stable order (same order as config within the project).
        results.sort_unstable_by_key(|a| a.hook.idx);

        self.file_modified |= modified_files;

        self.reporter.clear_completed();
        self.reporter
            .suspend(|| self.render_priority_group(&results, modified_files, hook_prefix))?;

        for RunResult { status, .. } in &results {
            let ok = if modified_files {
                false
            } else {
                status.as_bool()
            };
            self.success &= ok;
        }

        Ok(())
    }

    fn render_priority_group(
        &self,
        group_results: &[RunResult],
        group_modified_files: bool,
        hook_prefix: &str,
    ) -> Result<()> {
        // Only show a special group UI when the group failed due to file modifications.
        // Hooks in a priority group run in parallel, so we can't attribute modifications to a single hook.
        let show_group_ui = group_modified_files && group_results.len() > 1;
        let single_hook_modified_files = group_results.len() == 1 && group_modified_files;
        let group_output_prefix = if show_group_ui {
            format!("{hook_prefix}{}", "  │ ".dimmed())
        } else {
            String::new()
        };
        let detail_prefix = if show_group_ui {
            group_output_prefix.as_str()
        } else {
            hook_prefix
        };
        let group_separator = format!("{hook_prefix}{}", "  │".dimmed());

        if show_group_ui {
            self.status_printer.write(
                "Files were modified by following hooks",
                hook_prefix,
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
            let prefix = format!("{hook_prefix}{prefix}");

            // If a single hook modified files, treat it as failed.
            let status = if single_hook_modified_files && result.status == RunStatus::Success {
                RunStatus::Failed
            } else {
                result.status
            };

            self.status_printer
                .write(&result.hook.name, &prefix, status)?;

            if matches!(status, RunStatus::NoFiles) {
                continue;
            }

            let mut stdout = match status {
                RunStatus::Failed => self.printer.stdout_important(),
                _ => self.printer.stdout(),
            };

            if self.verbose || result.hook.verbose || status == RunStatus::Failed {
                writeln!(
                    stdout,
                    "{detail_prefix}{}",
                    format!("- hook id: {}", result.hook.id).dimmed()
                )?;
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
                if single_hook_modified_files {
                    writeln!(
                        stdout,
                        "{detail_prefix}{}",
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
                            writeln!(stdout, "{group_separator}")?;
                        } else {
                            writeln!(stdout)?;
                        }
                        let text = String::from_utf8_lossy(output);
                        for line in text.lines() {
                            if line.is_empty() {
                                if show_group_ui {
                                    writeln!(stdout, "{group_separator}")?;
                                } else {
                                    writeln!(stdout)?;
                                }
                            } else if show_group_ui {
                                writeln!(stdout, "{group_output_prefix}{line}")?;
                            } else {
                                writeln!(stdout, "{hook_prefix}  {line}")?;
                            }
                        }
                    }
                }
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

        if !self.success && show_diff_on_failure && self.file_modified {
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

        if self.success {
            Ok(ExitStatus::Success)
        } else {
            Ok(ExitStatus::Failure)
        }
    }
}

struct PriorityGroups {
    hooks: Vec<InstalledHook>,
}

impl PriorityGroups {
    fn new(hooks: Vec<InstalledHook>) -> Self {
        Self { hooks }
    }
}

impl Iterator for PriorityGroups {
    type Item = Vec<InstalledHook>;

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

enum ProjectHookInput<'a> {
    Files(ProjectFiles<'a>),
    MessageFile {
        hook_arg: PathBuf,
        tags: Option<TagSet>,
    },
}

impl<'a> ProjectHookInput<'a> {
    fn new(
        input: &'a RunInput,
        project: &Project,
        consumed_files: Option<&FxHashSet<&'a Path>>,
        newly_consumed_files: Option<&mut FxHashSet<&'a Path>>,
    ) -> Result<Self> {
        match input {
            RunInput::Files(files) => Ok(Self::Files(ProjectFiles::for_project(
                files.iter(),
                project,
                consumed_files,
                newly_consumed_files,
            ))),
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

    fn run_input_for_hook(&self, hook: &Hook, tag_cache: &FileTagCache<'a>) -> HookRunInput<'a> {
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
                            HookRunInput::with_filename(hook_arg.clone())
                        }
                    }
                } else {
                    HookRunInput::without_filenames(false)
                }
            }
        }
    }

    fn matches_hook(&self, hook: &Hook, tag_cache: &FileTagCache<'a>) -> bool {
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
    Filename(PathBuf),
    WithoutFilenames { matched: bool },
}

impl<'a> HookRunInput<'a> {
    fn with_filenames<I>(filenames: I) -> Self
    where
        I: IntoIterator<Item = &'a Path>,
    {
        Self::Filenames(filenames.into_iter().collect())
    }

    fn with_filename(filename: PathBuf) -> Self {
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

#[derive(Copy, Clone, Eq, PartialEq)]
enum RunStatus {
    Success,
    Failed,
    DryRun,
    NoFiles,
}

impl RunStatus {
    fn as_bool(self) -> bool {
        matches!(self, Self::Success | Self::NoFiles | Self::DryRun)
    }

    fn is_skipped(self) -> bool {
        matches!(self, Self::DryRun | Self::NoFiles)
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
        let dots = ".".repeat(dots).green().to_string();
        let line = format!("{prefix}{hook_name}{dots}{suffix}{status_line}");
        match status {
            RunStatus::Failed => {
                writeln!(self.printer.stdout_important(), "{line}")
            }
            _ => writeln!(self.printer.stdout(), "{line}"),
        }
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
    project_input: &ProjectHookInput<'_>,
    tag_cache: &FileTagCache<'_>,
    store: &Store,
    dry_run: bool,
    reporter: &HookRunReporter,
    semaphore: Rc<Semaphore>,
) -> Result<RunResult> {
    let _permit = if dry_run {
        None
    } else {
        Some(semaphore.acquire(1).await)
    };

    let mut input = project_input.run_input_for_hook(&hook, tag_cache);
    let matched = input.matched();
    let filename_count = input.filename_count();
    trace!(
        matched,
        filenames = filename_count,
        "Files for hook `{}` after filtering",
        hook.id,
    );

    if !matched && !hook.always_run {
        return Ok(RunResult::from_status(hook, RunStatus::NoFiles));
    }
    let start = std::time::Instant::now();
    input.shuffle();

    let (exit_status, hook_output) = if dry_run {
        (0, dry_run_hook(&hook, &input)?)
    } else {
        match &input {
            HookRunInput::Filenames(filenames) => {
                hook.language.run(store, &hook, filenames, reporter).await
            }
            HookRunInput::Filename(filename) => {
                let filenames = [filename.as_path()];
                hook.language.run(store, &hook, &filenames, reporter).await
            }
            HookRunInput::WithoutFilenames { .. } => {
                hook.language.run(store, &hook, &[], reporter).await
            }
        }
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
