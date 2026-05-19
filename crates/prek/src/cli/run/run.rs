use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use futures::stream::StreamExt;
use owo_colors::OwoColorize;
use prek_consts::env_vars::EnvVars;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PREK_TOML};
use rand::SeedableRng;
use rand::prelude::{SliceRandom, StdRng};
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
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

use super::install::{InstallCache, install_hooks};

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

    let mut tag_cache = FileTagCache::default();
    let installed_hooks = resolve_installed_hooks(
        store,
        printer,
        &workspace,
        &input,
        &mut tag_cache,
        &filtered_hooks,
    )
    .await?;

    run_hooks(
        &workspace,
        &input,
        &mut tag_cache,
        &installed_hooks,
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

/// Resolve hooks into the installed form expected by the runner.
///
/// Hooks that do not need an environment are returned as-is. Hooks that need an
/// environment first try the install cache; only cache misses are filtered
/// against the run input before installation.
async fn resolve_installed_hooks<'paths>(
    store: &Store,
    printer: Printer,
    workspace: &Workspace,
    input: &'paths RunInput,
    tag_cache: &mut FileTagCache<'paths>,
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
        filter_missing_hooks_to_install(workspace, input, tag_cache, &missing_env_hooks)?;
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
fn filter_missing_hooks_to_install<'paths>(
    workspace: &Workspace,
    input: &'paths RunInput,
    tag_cache: &mut FileTagCache<'paths>,
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
                let Some(hooks) = project_to_hooks.remove(project) else {
                    ProjectFiles::visit_project_files(
                        files.iter(),
                        project,
                        Some(&mut consumed_files),
                        |_| {},
                    );
                    continue;
                };

                let mut candidates = hooks
                    .into_iter()
                    .filter(|hook| Language::supported(hook.language))
                    .map(|hook| {
                        let matches = hook.always_run;
                        (hook, matches)
                    })
                    .collect::<Vec<_>>();

                ProjectFiles::visit_project_files(
                    files.iter(),
                    project,
                    Some(&mut consumed_files),
                    |project_file| {
                        for (hook, matches) in &mut candidates {
                            if *matches {
                                continue;
                            }

                            let hook_filter = HookFileFilter::new(hook);
                            if hook_filter.matches_project_file(&project_file, tag_cache) {
                                *matches = true;
                            }
                        }
                    },
                );

                for (hook, matches) in candidates {
                    if matches {
                        hooks_to_install.push(hook);
                    }
                }
            }
            RunInput::MessageFile(_) => {
                let Some(hooks) = project_to_hooks.remove(project) else {
                    continue;
                };

                let project_input = ProjectHookInput::new(input, project, None)?;
                for hook in hooks
                    .into_iter()
                    .filter(|hook| Language::supported(hook.language))
                {
                    if hook.always_run || project_input.has_match_for_hook(&hook, tag_cache) {
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
    tag_cache: &mut FileTagCache<'paths>,
    hooks: &[InstalledHook],
    store: &Store,
    show_diff_on_failure: bool,
    fail_fast: Option<bool>,
    dry_run: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    debug_assert!(!hooks.is_empty(), "No hooks to run");

    let mut session = HookRunSession::new(hooks, store, dry_run, verbose, printer);

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

    let projects_len = project_to_hooks.len();
    let mut consumed_files = FxHashSet::default();

    'outer: for project in workspace.all_projects() {
        let project_input = ProjectHookInput::new(input, project, Some(&mut consumed_files))?;
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

        session.render_project_header(project, projects_len)?;
        let mut prev_diff = git::get_diff(project.path()).await?;

        let project_fail_fast = fail_fast.or(project.config().fail_fast).unwrap_or(false);

        for group_hooks in PriorityGroups::new(hooks) {
            let group_results = session
                .run_priority_group(group_hooks, &project_input, tag_cache)
                .await?;

            let hook_fail_fast = session
                .finish_priority_group(group_results, project, &mut prev_diff)
                .await?;

            if !session.success && (project_fail_fast || hook_fail_fast) {
                break 'outer;
            }
        }
    }

    session.finish(workspace, show_diff_on_failure).await
}

#[allow(clippy::struct_excessive_bools)]
struct HookRunSession<'a> {
    store: &'a Store,
    reporter: HookRunReporter,
    status_printer: StatusPrinter,
    printer: Printer,
    dry_run: bool,
    verbose: bool,
    rendered_projects: usize,
    success: bool,
    file_modified: bool,
    has_unimplemented: bool,
}

impl<'a> HookRunSession<'a> {
    fn new(
        hooks: &[InstalledHook],
        store: &'a Store,
        dry_run: bool,
        verbose: bool,
        printer: Printer,
    ) -> Self {
        let status_printer = StatusPrinter::for_hooks(hooks, printer);
        let reporter = HookRunReporter::new(printer, status_printer.bar_len());

        Self {
            store,
            reporter,
            status_printer,
            printer,
            dry_run,
            verbose,
            rendered_projects: 0,
            success: true,
            file_modified: false,
            has_unimplemented: false,
        }
    }

    fn render_project_header(&mut self, project: &Project, projects_len: usize) -> Result<()> {
        if projects_len == 1 && project.is_root() {
            return Ok(());
        }

        self.reporter.suspend(|| {
            writeln!(
                self.status_printer.printer().stdout(),
                "{}{}",
                if self.rendered_projects == 0 {
                    ""
                } else {
                    "\n"
                },
                format!("Running hooks for `{}`:", project.to_string().cyan()).bold()
            )
        })?;
        self.rendered_projects += 1;

        Ok(())
    }

    async fn run_priority_group<'input, 'paths>(
        &self,
        group_hooks: Vec<InstalledHook>,
        input: &'input ProjectHookInput<'paths>,
        tag_cache: &mut FileTagCache<'paths>,
    ) -> Result<Vec<RunResult>>
    where
        'paths: 'input,
    {
        debug!(
            "Running priority group with priority {} with concurrency {}: {:?}",
            group_hooks[0].priority,
            *CONCURRENCY,
            group_hooks.iter().map(|h| &h.id).collect::<Vec<_>>()
        );

        let mut runs = futures::stream::iter(group_hooks)
            .map(|hook| {
                let hook_input = input.for_hook(&hook, tag_cache);
                trace!(
                    matched = hook_input.has_matching_files,
                    filenames = hook_input.filenames.len(),
                    "Files for hook `{}` after filtering",
                    hook.id,
                );
                run_hook(hook, hook_input, self.store, self.dry_run, &self.reporter)
            })
            .buffer_unordered(*CONCURRENCY);

        let mut group_results = Vec::new();
        while let Some(result) = runs.next().await {
            group_results.push(result?);
        }
        Ok(group_results)
    }

    async fn finish_priority_group(
        &mut self,
        mut group_results: Vec<RunResult>,
        project: &Project,
        prev_diff: &mut Vec<u8>,
    ) -> Result<bool> {
        // Print results in a stable order (same order as config within the project).
        group_results.sort_unstable_by_key(|a| a.hook.idx);

        // Check if any files were modified by this group of hooks.
        let all_skipped = group_results.iter().all(|r| r.status.is_skipped());
        let group_modified_files = if !all_skipped {
            let curr_diff = git::get_diff(project.path()).await?;
            let group_modified_files = curr_diff != *prev_diff;
            *prev_diff = curr_diff;
            group_modified_files
        } else {
            false
        };

        self.file_modified |= group_modified_files;

        self.reporter.clear_completed();
        self.reporter
            .suspend(|| self.render_priority_group(&group_results, group_modified_files))?;

        let mut hook_fail_fast = false;
        for RunResult { hook, status, .. } in &group_results {
            self.has_unimplemented |= status.is_unimplemented();

            let ok = if group_modified_files {
                false
            } else {
                status.as_bool()
            };
            self.success &= ok;

            if !ok && hook.fail_fast {
                hook_fail_fast = true;
            }
        }

        Ok(hook_fail_fast)
    }

    fn render_priority_group(
        &self,
        group_results: &[RunResult],
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
            self.status_printer.write(
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

            self.status_printer
                .write(&result.hook.name, prefix, status)?;

            if matches!(status, RunStatus::NoFiles | RunStatus::Unimplemented) {
                continue;
            }

            let mut stdout = match status {
                RunStatus::Failed => self.printer.stdout_important(),
                _ => self.printer.stdout(),
            };

            if self.verbose || result.hook.verbose || status == RunStatus::Failed {
                writeln!(
                    stdout,
                    "{group_prefix}{}",
                    format!("- hook id: {}", result.hook.id).dimmed()
                )?;
                if self.verbose || result.hook.verbose {
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
                            } else if show_group_ui {
                                writeln!(stdout, "{group_prefix}{line}")?;
                            } else {
                                writeln!(stdout, "  {line}")?;
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

        if self.has_unimplemented {
            warn_user!(
                "Some hooks were skipped because their languages are unimplemented.\nWe're working hard to support more languages. Check out current support status at {}.",
                "https://prek.j178.dev/languages/".cyan().underline()
            );
        }

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

        if self.success {
            Ok(ExitStatus::Success)
        } else {
            Ok(ExitStatus::Failure)
        }
    }
}

struct PriorityGroups {
    hooks: Vec<InstalledHook>,
    idx: usize,
}

impl PriorityGroups {
    fn new(hooks: Vec<InstalledHook>) -> Self {
        Self { hooks, idx: 0 }
    }
}

impl Iterator for PriorityGroups {
    type Item = Vec<InstalledHook>;

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
                PassFilenames::None => HookRunInput::without_filenames(
                    project_files.has_match_for_hook(hook, tag_cache),
                ),
                PassFilenames::All | PassFilenames::Limited(_) => {
                    HookRunInput::with_filenames(project_files.for_hook(hook, tag_cache))
                }
            },
            Self::MessageFile { hook_arg, .. } => {
                if self.has_match_for_hook(hook, tag_cache) {
                    match hook.pass_filenames {
                        PassFilenames::None => HookRunInput::without_filenames(true),
                        PassFilenames::All | PassFilenames::Limited(_) => {
                            HookRunInput::with_filenames(vec![hook_arg.as_path()])
                        }
                    }
                } else {
                    HookRunInput::without_filenames(false)
                }
            }
        }
    }

    fn has_match_for_hook(&self, hook: &Hook, tag_cache: &mut FileTagCache<'a>) -> bool {
        match self {
            Self::Files(project_files) => project_files.has_match_for_hook(hook, tag_cache),
            Self::MessageFile {
                absolute_path,
                hook_arg,
            } => {
                // `commit-msg` and `prepare-commit-msg` receive Git's special message file,
                // which can live outside a project root, so it bypasses project ownership
                // filtering. Hook-level `files`/`exclude`/`types` filters still apply.
                let hook_filter = HookFileFilter::new(hook);
                hook_filter.matches_filename(hook_arg)
                    && hook_filter.matches_tags(tag_cache.tags(absolute_path))
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

    fn without_filenames(has_matching_files: bool) -> Self {
        Self {
            has_matching_files,
            filenames: Vec::new(),
        }
    }

    fn into_filenames(mut self) -> Vec<&'a Path> {
        // Shuffle the files so that they more evenly fill out the xargs
        // partitions, but do it deterministically in case a hook cares about ordering.
        const SEED: u64 = 1_542_676_187;
        let mut rng = StdRng::seed_from_u64(SEED);
        self.filenames.shuffle(&mut rng);
        self.filenames
    }
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
    input: HookRunInput<'_>,
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

    let filenames = input.into_filenames();

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
