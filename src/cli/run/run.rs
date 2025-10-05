use std::fmt::Write as _;
use std::io::Write;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use owo_colors::{OwoColorize, Style};
use rand::SeedableRng;
use rand::prelude::{SliceRandom, StdRng};
use rustc_hash::FxHashMap;
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tracing::{debug, trace};
use unicode_width::UnicodeWidthStr;

use constants::env_vars::EnvVars;

use crate::cli::reporter::{HookInitReporter, HookInstallReporter};
use crate::cli::run::keeper::WorkTreeKeeper;
use crate::cli::run::{CollectOptions, FileFilter, Selectors, collect_files};
use crate::cli::{ExitStatus, RunExtraArgs};
use crate::config::{Language, Stage};
use crate::fs::CWD;
use crate::git;
use crate::git::GIT_ROOT;
use crate::hook::{Hook, InstalledHook};
use crate::printer::{Printer, Stdout};
use crate::run::{CONCURRENCY, USE_COLOR};
use crate::store::{STORE, Store};
use crate::workspace::{Project, Workspace};

#[allow(clippy::too_many_arguments, clippy::fn_params_excessive_bools)]
pub(crate) async fn run(
    config: Option<PathBuf>,
    includes: Vec<String>,
    skips: Vec<String>,
    hook_stage: Stage,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<String>,
    directories: Vec<String>,
    last_commit: bool,
    show_diff_on_failure: bool,
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
    if hook_stage == Stage::PostCheckout
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
    let mut workspace = Workspace::discover(workspace_root, config, Some(&selectors), refresh)?;

    if should_stash {
        workspace.check_configs_staged().await?;
    }

    let store = STORE.as_ref()?;
    let reporter = HookInitReporter::from(printer);
    let lock = store.lock_async().await?;

    let hooks = workspace.init_hooks(store, Some(&reporter)).await?;
    let filtered_hooks: Vec<_> = hooks
        .into_iter()
        .filter(|h| selectors.matches_hook(h))
        .map(Arc::new)
        .collect();

    selectors.report_unused();

    if filtered_hooks.is_empty() {
        writeln!(
            printer.stderr(),
            "{}: No hooks found after filtering with the given selectors",
            "error".red().bold(),
        )?;
        return Ok(ExitStatus::Failure);
    }

    let filtered_hooks = filtered_hooks
        .into_iter()
        .filter(|h| h.stages.contains(hook_stage))
        .collect::<Vec<_>>();

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
        _guard = Some(WorkTreeKeeper::clean(store, workspace.root()).await?);
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
    .await?;

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

        if let Some(ref source) = args.prepare_commit_message_source {
            std::env::set_var("PRE_COMMIT_COMMIT_MSG_SOURCE", source.clone());
        }
        if let Some(ref object) = args.commit_object_name {
            std::env::set_var("PRE_COMMIT_COMMIT_OBJECT_NAME", object.clone());
        }
        if let Some(from_ref) = from_ref {
            std::env::set_var("PRE_COMMIT_ORIGIN", from_ref.clone());
            std::env::set_var("PRE_COMMIT_FROM_REF", from_ref.clone());
        }
        if let Some(to_ref) = to_ref {
            std::env::set_var("PRE_COMMIT_SOURCE", to_ref.clone());
            std::env::set_var("PRE_COMMIT_TO_REF", to_ref.clone());
        }
        if let Some(ref upstream) = args.pre_rebase_upstream {
            std::env::set_var("PRE_COMMIT_PRE_REBASE_UPSTREAM", upstream.clone());
        }
        if let Some(ref branch) = args.pre_rebase_branch {
            std::env::set_var("PRE_COMMIT_PRE_REBASE_BRANCH", branch.clone());
        }
        if let Some(ref branch) = args.local_branch {
            std::env::set_var("PRE_COMMIT_LOCAL_BRANCH", branch.clone());
        }
        if let Some(ref branch) = args.remote_branch {
            std::env::set_var("PRE_COMMIT_REMOTE_BRANCH", branch.clone());
        }
        if let Some(ref name) = args.remote_name {
            std::env::set_var("PRE_COMMIT_REMOTE_NAME", name.clone());
        }
        if let Some(ref url) = args.remote_url {
            std::env::set_var("PRE_COMMIT_REMOTE_URL", url.clone());
        }
        if let Some(ref checkout) = args.checkout_type {
            std::env::set_var("PRE_COMMIT_CHECKOUT_TYPE", checkout.clone());
        }
        if args.is_squash_merge {
            std::env::set_var("PRE_COMMIT_SQUASH_MERGE", "1");
        }
        if let Some(ref command) = args.rewrite_command {
            std::env::set_var("PRE_COMMIT_REWRITE_COMMAND", command.clone());
        }
    }
}

pub async fn install_hooks(
    hooks: Vec<Arc<Hook>>,
    store: &Store,
    reporter: &HookInstallReporter,
) -> Result<Vec<InstalledHook>> {
    let num_hooks = hooks.len();
    let mut installed_hooks = Vec::with_capacity(hooks.len());
    let store_hooks = Rc::new(store.installed_hooks().await);

    // Group hooks by language to enable parallel installation across different languages.
    let mut hooks_by_language = FxHashMap::default();
    for hook in hooks {
        hooks_by_language
            .entry(hook.language)
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
                    // Find a matching installed hook environment.
                    if let Some(info) = store_hooks
                        .iter()
                        .chain(newly_installed.iter().filter_map(|h| {
                            if let InstalledHook::Installed { info, .. } = h {
                                Some(info)
                            } else {
                                None
                            }
                        }))
                        .find(|info| info.matches(&hook))
                    {
                        debug!(
                            "Found installed environment for hook `{}` at `{}`",
                            &hook,
                            info.env_path.display()
                        );
                        hook_envs.push(InstalledHook::Installed {
                            hook,
                            info: info.clone(),
                        });
                        continue;
                    }

                    let _permit = semaphore.acquire().await.unwrap();
                    debug!("No matching environment found for hook `{hook}`, installing...");

                    let installed_hook = hook
                        .language
                        .install(hook.clone(), store, reporter)
                        .await
                        .context(format!("Failed to install hook `{hook}`"))?;

                    installed_hook
                        .mark_as_installed(store)
                        .await
                        .context(format!("Failed to mark hook `{hook}` as installed"))?;

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

    while let Some(result) = futures.next().await {
        installed_hooks.extend(result?);
    }
    reporter.on_complete();

    debug_assert_eq!(
        num_hooks,
        installed_hooks.len(),
        "Number of hooks installed should match the number of hooks provided"
    );

    Ok(installed_hooks)
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

    // Sort projects by their depth in the workspace.
    let mut project_to_hooks: Vec<_> = project_to_hooks.into_iter().collect();
    project_to_hooks.sort_by_key(|(_, hooks)| hooks[0].project().idx());

    let projects_len = project_to_hooks.len();
    let mut first = true;
    let mut file_modified = false;

    // Hooks might modify the files, so they must be run sequentially.
    'outer: for (_, mut hooks) in project_to_hooks {
        hooks.sort_by_key(|h| h.idx);

        let project = hooks[0].project();
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

        let fail_fast = project.config().fail_fast.unwrap_or(false);

        let filter = FileFilter::for_project(filenames.iter(), project);
        trace!(
            "Files for project `{project}` after filtered: {}",
            filter.len()
        );

        let mut hook_succeed;
        for hook in hooks {
            (hook_succeed, diff, file_modified) =
                run_hook(hook, &filter, store, diff, verbose, dry_run, &printer).await?;

            success &= hook_succeed;
            if !success && (fail_fast || hook.fail_fast) {
                break 'outer;
            }
        }
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

async fn run_hook(
    hook: &InstalledHook,
    filter: &FileFilter<'_>,
    store: &Store,
    diff: Vec<u8>,
    verbose: bool,
    dry_run: bool,
    printer: &StatusPrinter,
) -> Result<(bool, Vec<u8>, bool)> {
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
        return Ok((true, diff, false));
    }

    if !Language::supported(hook.language) {
        printer.write_skipped(
            &hook.name,
            StatusPrinter::UNIMPLEMENTED,
            Style::new().black().on_yellow(),
        )?;
        return Ok((true, diff, false));
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
            .context(format!("Failed to run hook `{hook}`"))?
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
                    "{}",
                    textwrap::indent(&String::from_utf8_lossy(output), "  ").dimmed()
                )?;
            }
        }
    }

    Ok((success, new_diff, file_modified))
}
