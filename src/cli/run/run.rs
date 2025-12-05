use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use owo_colors::{OwoColorize, Style};
use prek_consts::env_vars::EnvVars;
use rand::SeedableRng;
use rand::prelude::{SliceRandom, StdRng};
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::io::AsyncWriteExt;
use tokio::sync::{OnceCell, Semaphore};
use tracing::{debug, trace, warn};
use unicode_width::UnicodeWidthStr;

use crate::cli::reporter::{HookInitReporter, HookInstallReporter};
use crate::cli::run::keeper::WorkTreeKeeper;
use crate::cli::run::{CollectOptions, FileFilter, Selectors, collect_files};
use crate::cli::{ExitStatus, RunExtraArgs};
use crate::config::{Language, Stage};
use crate::fs::CWD;
use crate::git::GIT_ROOT;
use crate::hook::{Hook, InstallInfo, InstalledHook, Repo};
use crate::printer::{Printer, Stdout};
use crate::run::{CONCURRENCY, USE_COLOR};
use crate::store::Store;
use crate::workspace::{Project, Workspace};
use crate::{git, warn_user};

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
    fail_fast: bool,
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

    let reporter = HookInitReporter::from(printer);
    let lock = store.lock_async().await?;

    let hooks = workspace
        .init_hooks(store, Some(&reporter))
        .await
        .context("Failed to init hooks")?;
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
                "\n{} If you just added new `{}`, try rerun your command with the `{}` flag to rescan the workspace.",
                "hint:".bold().yellow(),
                ".pre-commit-config.yaml".cyan(),
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
        writeln!(
            printer.stderr(),
            "{}: No hooks found for stage `{}` after filtering",
            "error".red().bold(),
            hook_stage.cyan()
        )?;
        return Ok(ExitStatus::Failure);
    }

    debug!(
        "Hooks going to run: {:?}",
        filtered_hooks.iter().map(|h| &h.id).collect::<Vec<_>>()
    );
    let reporter = HookInstallReporter::from(printer);
    let installed_hooks = install_hooks(filtered_hooks, store, &reporter).await?;

    // Release the store lock.
    drop(lock);

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

    let filenames = collect_files(
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
        &installed_hooks,
        filenames,
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

#[derive(Debug)]
struct LazyInstallInfo {
    info: Arc<InstallInfo>,
    health: OnceCell<bool>,
}

impl LazyInstallInfo {
    fn new(info: Arc<InstallInfo>) -> Self {
        Self {
            info,
            health: OnceCell::new(),
        }
    }

    fn matches(&self, hook: &Hook) -> bool {
        self.info.matches(hook)
    }

    fn info(&self) -> Arc<InstallInfo> {
        self.info.clone()
    }

    async fn ensure_healthy(&self) -> bool {
        let info = self.info.clone();
        *self
            .health
            .get_or_init(|| async move {
                match info.check_health().await {
                    Ok(()) => true,
                    Err(err) => {
                        warn!(
                            %err,
                            path = %info.env_path.display(),
                            "Skipping unhealthy installed hook"
                        );
                        false
                    }
                }
            })
            .await
    }
}

pub async fn install_hooks(
    hooks: Vec<Arc<Hook>>,
    store: &Store,
    reporter: &HookInstallReporter,
) -> Result<Vec<InstalledHook>> {
    let num_hooks = hooks.len();
    let mut result = Vec::with_capacity(hooks.len());

    let store_hooks = Rc::new(
        store
            .installed_hooks()
            .await
            .into_iter()
            .map(LazyInstallInfo::new)
            .collect::<Vec<_>>(),
    );

    // Group hooks by language to enable parallel installation across different languages.
    let mut hooks_by_language = FxHashMap::default();
    for hook in hooks {
        let mut language = hook.language;
        if hook.language == Language::Pygrep {
            // Treat `pygrep` hooks as `python` hooks for installation purposes.
            // They share the same installation logic.
            language = Language::Python;
        }
        hooks_by_language
            .entry(language)
            .or_insert_with(Vec::new)
            .push(hook);
    }

    let mut futures = FuturesUnordered::new();
    let semaphore = Arc::new(Semaphore::new(*CONCURRENCY));

    for (_, hooks) in hooks_by_language {
        let semaphore = semaphore.clone();
        let partitions = partition_hooks(&hooks);

        for hooks in partitions {
            let semaphore = semaphore.clone();
            let store_hooks = store_hooks.clone();

            futures.push(async move {
                let mut hook_envs = Vec::with_capacity(hooks.len());
                let mut newly_installed = Vec::new();

                for hook in hooks {
                    if matches!(hook.repo(), Repo::Meta { .. } | Repo::Builtin { .. }) {
                        debug!(
                            "Hook `{}` is a meta or builtin hook, no installation needed",
                            &hook
                        );
                        hook_envs.push(InstalledHook::NoNeedInstall(hook));
                        continue;
                    }

                    let mut matched_info = None;

                    for env in &newly_installed {
                        if let InstalledHook::Installed { info, .. } = env {
                            if info.matches(&hook) {
                                matched_info = Some(info.clone());
                                break;
                            }
                        }
                    }

                    if matched_info.is_none() {
                        for env in store_hooks.iter() {
                            if env.matches(&hook) {
                                if env.ensure_healthy().await {
                                    matched_info = Some(env.info());
                                    break;
                                }
                            }
                        }
                    }

                    if let Some(info) = matched_info {
                        debug!(
                            "Found installed environment for hook `{}` at `{}`",
                            &hook,
                            info.env_path.display()
                        );
                        hook_envs.push(InstalledHook::Installed { hook, info });
                        continue;
                    }

                    let _permit = semaphore.acquire().await.unwrap();

                    let installed_hook = hook
                        .language
                        .install(hook.clone(), store, reporter)
                        .await
                        .with_context(|| format!("Failed to install hook `{hook}`"))?;

                    installed_hook
                        .mark_as_installed(store)
                        .await
                        .with_context(|| format!("Failed to mark hook `{hook}` as installed"))?;

                    match &installed_hook {
                        InstalledHook::Installed { info, .. } => {
                            debug!("Installed hook `{hook}` in `{}`", info.env_path.display());
                        }
                        InstalledHook::NoNeedInstall { .. } => {
                            debug!("Hook `{hook}` does not need installation");
                        }
                    }

                    newly_installed.push(installed_hook);
                }

                // Add newly installed hooks to the list.
                hook_envs.extend(newly_installed);
                anyhow::Ok(hook_envs)
            });
        }
    }

    while let Some(hooks) = futures.next().await {
        result.extend(hooks?);
    }
    reporter.on_complete();

    debug_assert_eq!(
        num_hooks,
        result.len(),
        "Number of hooks installed should match the number of hooks provided"
    );

    Ok(result)
}

/// Partition hooks into groups where hooks in the same group have same dependencies.
/// Hooks in different groups can be installed in parallel.
fn partition_hooks(hooks: &[Arc<Hook>]) -> Vec<Vec<Arc<Hook>>> {
    if hooks.is_empty() {
        return vec![];
    }

    let n = hooks.len();
    let mut visited = vec![false; n];
    let mut groups = Vec::new();

    // DFS to find all connected sets
    #[allow(clippy::items_after_statements)]
    fn dfs(
        index: usize,
        hooks: &[Arc<Hook>],
        visited: &mut [bool],
        current_group: &mut Vec<usize>,
    ) {
        visited[index] = true;
        current_group.push(index);

        for i in 0..hooks.len() {
            if !visited[i] && hooks[index].dependencies() == hooks[i].dependencies() {
                dfs(i, hooks, visited, current_group);
            }
        }
    }

    // Find all connected components
    for i in 0..n {
        if !visited[i] {
            let mut current_group = Vec::new();
            dfs(i, hooks, &mut visited, &mut current_group);

            // Convert indices back to actual sets
            let group_sets: Vec<Arc<Hook>> = current_group
                .into_iter()
                .map(|idx| hooks[idx].clone())
                .collect();

            groups.push(group_sets);
        }
    }

    groups
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

    fn for_hooks(hooks: &[InstalledHook], printer: Printer) -> Self {
        let columns = Self::calculate_columns(hooks);
        Self { printer, columns }
    }

    fn calculate_columns(hooks: &[InstalledHook]) -> usize {
        let name_len = hooks
            .iter()
            .map(|hook| hook.name.width_cjk())
            .max()
            .unwrap_or(0);
        std::cmp::max(
            80,
            name_len + 3 + Self::NO_FILES.len() + 1 + Self::SKIPPED.len(),
        )
    }

    fn write_skipped(
        &self,
        hook_name: &str,
        reason: &str,
        style: Style,
    ) -> Result<(), std::fmt::Error> {
        let dots = self.columns - hook_name.width_cjk() - Self::SKIPPED.len() - reason.len() - 1;
        let line = format!(
            "{hook_name}{}{}{}",
            ".".repeat(dots),
            reason,
            Self::SKIPPED.style(style)
        );
        writeln!(self.printer.stdout(), "{line}")
    }

    fn write_running(&self, hook_name: &str, important: bool) -> Result<(), std::fmt::Error> {
        write!(
            if important {
                self.printer.stdout_important()
            } else {
                self.printer.stdout()
            },
            "{}{}",
            hook_name,
            ".".repeat(self.columns - hook_name.width_cjk() - Self::PASSED.len() - 1)
        )
    }

    fn write_dry_run(&self) -> Result<(), std::fmt::Error> {
        writeln!(self.printer.stdout(), "{}", Self::DRY_RUN.on_yellow())
    }

    fn write_passed(&self) -> Result<(), std::fmt::Error> {
        writeln!(self.printer.stdout(), "{}", Self::PASSED.on_green())
    }

    fn write_failed(&self) -> Result<(), std::fmt::Error> {
        writeln!(self.printer.stdout_important(), "{}", Self::FAILED.on_red())
    }

    fn stdout(&self) -> Stdout {
        self.printer.stdout()
    }

    fn stdout_important(&self) -> Stdout {
        self.printer.stdout_important()
    }
}

/// Run all hooks.
#[allow(clippy::fn_params_excessive_bools)]
async fn run_hooks(
    workspace: &Workspace,
    hooks: &[InstalledHook],
    filenames: Vec<PathBuf>,
    store: &Store,
    show_diff_on_failure: bool,
    fail_fast: bool,
    dry_run: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    debug_assert!(!hooks.is_empty(), "No hooks to run");

    let printer = StatusPrinter::for_hooks(hooks, printer);

    let mut success = true;

    // Group hooks by project to run them in order of their depth in the workspace.
    #[allow(clippy::mutable_key_type)]
    let mut project_to_hooks: FxHashMap<&Project, Vec<&InstalledHook>> = FxHashMap::default();
    for hook in hooks {
        project_to_hooks
            .entry(hook.project())
            .or_default()
            .push(hook);
    }

    let projects_len = project_to_hooks.len();
    let mut first = true;
    let mut file_modified = false;
    let mut has_unimplemented = false;

    // Track files that have been consumed by orphan projects.
    let mut consumed_files = FxHashSet::default();

    // Hooks might modify the files, so they must be run sequentially.
    'outer: for project in workspace.all_projects() {
        let filter = FileFilter::for_project(filenames.iter(), project, Some(&mut consumed_files));

        let Some(mut hooks) = project_to_hooks.remove(project) else {
            continue;
        };

        hooks.sort_by_key(|h| h.idx);

        if projects_len > 1 || !project.is_root() {
            writeln!(
                printer.stdout(),
                "{}{}:",
                if first { "" } else { "\n" },
                format!("Running hooks for `{}`", project.to_string().cyan()).bold()
            )?;
            first = false;
        }
        let mut diff = git::get_diff(project.path()).await?;

        let fail_fast = fail_fast || project.config().fail_fast.unwrap_or(false);

        trace!(
            "Files for project `{project}` after filtered: {}",
            filter.len()
        );

        for hook in hooks {
            let result = run_hook(hook, &filter, store, diff, verbose, dry_run, &printer).await?;
            diff = result.new_diff;
            file_modified |= result.file_modified;
            has_unimplemented |= result.status.is_unimplemented();

            success &= result.status.as_bool();
            if !success && (fail_fast || hook.fail_fast) {
                break 'outer;
            }
        }
    }

    if has_unimplemented {
        warn_user!(
            "Some hooks were skipped because their languages are unimplemented.\nWe're working hard to support more languages. Check out current support status at {}.",
            "https://prek.j178.dev/todo/#language-support-status"
                .cyan()
                .underline()
        );
    }

    if !success && show_diff_on_failure && file_modified {
        if EnvVars::is_set(EnvVars::CI) {
            writeln!(
                printer.stdout(),
                "{}",
                indoc::formatdoc! {
                    "\n{}: Some hooks made changes to the files.
                    If you are seeing this message in CI, reproduce locally with: `{}`
                    To run prek as part of git workflow, use `{}` to set up git hooks.\n",
                    "Hint".yellow().bold(),
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

/// Shuffle the files so that they more evenly fill out the xargs
/// partitions, but do it deterministically in case a hook cares about ordering.
fn shuffle<T>(filenames: &mut [T]) {
    const SEED: u64 = 1_542_676_187;
    let mut rng = StdRng::seed_from_u64(SEED);
    filenames.shuffle(&mut rng);
}

enum RunStatus {
    Success,
    Failed,
    Skipped,
    Unimplemented,
}

impl RunStatus {
    fn from_bool(success: bool) -> Self {
        if success { Self::Success } else { Self::Failed }
    }

    fn as_bool(&self) -> bool {
        matches!(self, Self::Success | Self::Skipped | Self::Unimplemented)
    }

    fn is_unimplemented(&self) -> bool {
        matches!(self, Self::Unimplemented)
    }
}

struct RunResult {
    status: RunStatus,
    new_diff: Vec<u8>,
    file_modified: bool,
}

async fn run_hook(
    hook: &InstalledHook,
    filter: &FileFilter<'_>,
    store: &Store,
    diff: Vec<u8>,
    verbose: bool,
    dry_run: bool,
    printer: &StatusPrinter,
) -> Result<RunResult> {
    let mut filenames = filter.for_hook(hook);
    trace!(
        "Files for hook `{}` after filtered: {}",
        hook.id,
        filenames.len()
    );

    if filenames.is_empty() && !hook.always_run {
        printer.write_skipped(
            &hook.name,
            StatusPrinter::NO_FILES,
            Style::new().black().on_cyan(),
        )?;
        return Ok(RunResult {
            status: RunStatus::Skipped,
            new_diff: diff,
            file_modified: false,
        });
    }

    if !Language::supported(hook.language) {
        printer.write_skipped(
            &hook.name,
            StatusPrinter::UNIMPLEMENTED,
            Style::new().black().on_yellow(),
        )?;
        return Ok(RunResult {
            status: RunStatus::Unimplemented,
            new_diff: diff,
            file_modified: false,
        });
    }

    printer.write_running(&hook.name, false)?;
    std::io::stdout().flush()?;

    let start = std::time::Instant::now();

    let filenames = if hook.pass_filenames {
        shuffle(&mut filenames);
        filenames
    } else {
        vec![]
    };

    let (status, output) = if dry_run {
        let mut output = Vec::new();
        if !filenames.is_empty() {
            writeln!(
                output,
                "`{}` would be run on {} files:",
                hook,
                filenames.len()
            )?;
        }
        for filename in &filenames {
            writeln!(output, "- {}", filename.to_string_lossy())?;
        }
        (0, output)
    } else {
        hook.language
            .run(hook, &filenames, store)
            .await
            .with_context(|| format!("Failed to run hook `{hook}`"))?
    };

    let duration = start.elapsed();

    let new_diff = git::get_diff(hook.work_dir()).await?;
    let file_modified = diff != new_diff;
    let success = status == 0 && !file_modified;
    if dry_run {
        printer.write_dry_run()?;
    } else if success {
        printer.write_passed()?;
    } else {
        // If the printer is in quiet mode, the running line was not printed.
        // Reprint it here before printing the failure.
        if printer.stdout() == Stdout::Disabled {
            printer.write_running(&hook.name, true)?;
        }
        printer.write_failed()?;
    }

    if verbose || hook.verbose || !success {
        let mut stdout = if success {
            printer.stdout()
        } else {
            printer.stdout_important()
        };

        writeln!(stdout, "{}", format!("- hook id: {}", hook.id).dimmed())?;
        if verbose || hook.verbose {
            writeln!(
                stdout,
                "{}",
                format!("- duration: {:.2?}s", duration.as_secs_f64()).dimmed()
            )?;
        }
        if status != 0 {
            writeln!(stdout, "{}", format!("- exit code: {status}").dimmed())?;
        }
        if file_modified {
            writeln!(stdout, "{}", "- files were modified by this hook".dimmed())?;
        }

        let output = output.trim_ascii();
        if !output.is_empty() {
            if let Some(file) = hook.log_file.as_deref() {
                let mut file = fs_err::tokio::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(file)
                    .await?;
                file.write_all(output).await?;
                file.sync_all().await?;
            } else {
                writeln!(
                    stdout,
                    "\n{}",
                    textwrap::indent(&String::from_utf8_lossy(output), "  ")
                )?;
            }
        }
    }

    Ok(RunResult {
        status: RunStatus::from_bool(success),
        new_diff,
        file_modified,
    })
}
