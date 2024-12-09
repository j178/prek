use std::cmp::max;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anstream::ColorChoice;
use anyhow::Result;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;
use owo_colors::{OwoColorize, Style};
use rand::prelude::{SliceRandom, StdRng};
use rand::SeedableRng;
use tracing::{debug, trace};
use unicode_width::UnicodeWidthStr;

use crate::cli::reporter::{HookInitReporter, HookInstallReporter};
use crate::cli::run::keeper::WorkTreeKeeper;
use crate::cli::run::{get_filenames, FileFilter, FileOptions};
use crate::cli::{ExitStatus, RunExtraArgs};
use crate::config::Stage;
use crate::fs::Simplified;
use crate::git;
use crate::git::{get_diff, git_cmd};
use crate::hook::{Hook, Project};
use crate::printer::Printer;
use crate::store::Store;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    config: Option<PathBuf>,
    hook_id: Option<String>,
    hook_stage: Option<Stage>,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<PathBuf>,
    show_diff_on_failure: bool,
    extra_args: RunExtraArgs,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    // Prevent recursive post-checkout hooks.
    if matches!(hook_stage, Some(Stage::PostCheckout))
        && std::env::var_os("_PRE_COMMIT_SKIP_POST_CHECKOUT").is_some()
    {
        return Ok(ExitStatus::Success);
    }

    let should_stash = !all_files && files.is_empty();

    // Check if we have unresolved merge conflict files and fail fast.
    if should_stash && git::has_unmerged_paths().await? {
        writeln!(
            printer.stderr(),
            "You have unmerged paths. Resolve them before running prefligit."
        )?;
        return Ok(ExitStatus::Failure);
    }

    let config_file = Project::find_config_file(config)?;
    if should_stash && config_not_staged(&config_file).await? {
        writeln!(
            printer.stderr(),
            "Your pre-commit configuration is unstaged.\n`git add {}` to fix this.",
            &config_file.user_display()
        )?;
        return Ok(ExitStatus::Failure);
    }

    // Set env vars for hooks.
    let env_vars = fill_envs(from_ref.as_ref(), to_ref.as_ref(), &extra_args);

    let mut project = Project::new(config_file)?;
    let store = Store::from_settings()?.init()?;

    let reporter = HookInitReporter::from(printer);

    let lock = store.lock_async().await?;
    let hooks = project.init_hooks(&store, Some(&reporter)).await?;

    let hooks: Vec<_> = hooks
        .into_iter()
        .filter(|h| {
            if let Some(ref hook) = hook_id {
                &h.id == hook || &h.alias == hook
            } else {
                true
            }
        })
        .filter(|h| {
            if let Some(stage) = hook_stage {
                h.stages.contains(&stage)
            } else {
                true
            }
        })
        .collect();

    if hooks.is_empty() && hook_id.is_some() {
        if let Some(hook_stage) = hook_stage {
            writeln!(
                printer.stderr(),
                "No hook found for id `{}` and stage `{}`",
                hook_id.unwrap().cyan(),
                hook_stage.cyan()
            )?;
        } else {
            writeln!(
                printer.stderr(),
                "No hook found for id `{}`",
                hook_id.unwrap().cyan()
            )?;
        }
        return Ok(ExitStatus::Failure);
    }

    let skips = get_skips();
    let to_run = hooks
        .iter()
        .filter(|h| !skips.contains(&h.id) && !skips.contains(&h.alias))
        .cloned()
        .collect::<Vec<_>>();

    debug!(
        "Hooks going to run: {:?}",
        to_run.iter().map(|h| &h.id).collect::<Vec<_>>()
    );
    let reporter = HookInstallReporter::from(printer);
    install_hooks(&to_run, &reporter).await?;
    drop(lock);

    // Clear any unstaged changes from the git working directory.
    let mut _guard = None;
    if should_stash {
        _guard = Some(WorkTreeKeeper::clean(&store).await?);
    }

    let filenames = get_filenames(FileOptions {
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        commit_msg_filename: extra_args.commit_msg_filename.clone(),
    })
    .await?;

    let filter = FileFilter::new(
        &filenames,
        project.config().files.as_deref(),
        project.config().exclude.as_deref(),
    )?;
    trace!("Files after filtered: {}", filter.len());

    run_hooks(
        &hooks,
        &skips,
        &filter,
        env_vars,
        project.config().fail_fast.unwrap_or(false),
        show_diff_on_failure,
        verbose,
        printer,
    )
    .await
}

async fn config_not_staged(config: &Path) -> Result<bool> {
    let status = git::git_cmd("git diff")?
        .arg("diff")
        .arg("--quiet") // Implies --exit-code
        .arg("--no-ext-diff")
        .arg(config)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .check(false)
        .status()
        .await?;

    Ok(!status.success())
}

fn fill_envs(
    from_ref: Option<&String>,
    to_ref: Option<&String>,
    args: &RunExtraArgs,
) -> HashMap<&'static str, String> {
    let mut env = HashMap::new();
    env.insert("PRE_COMMIT", "1".into());

    if let Some(ref source) = args.prepare_commit_message_source {
        env.insert("PRE_COMMIT_COMMIT_MSG_SOURCE", source.clone());
    }
    if let Some(ref object) = args.commit_object_name {
        env.insert("PRE_COMMIT_COMMIT_OBJECT_NAME", object.clone());
    }
    if let Some(from_ref) = from_ref {
        env.insert("PRE_COMMIT_ORIGIN", from_ref.clone());
        env.insert("PRE_COMMIT_FROM_REF", from_ref.clone());
    }
    if let Some(to_ref) = to_ref {
        env.insert("PRE_COMMIT_SOURCE", to_ref.clone());
        env.insert("PRE_COMMIT_TO_REF", to_ref.clone());
    }
    if let Some(ref upstream) = args.pre_rebase_upstream {
        env.insert("PRE_COMMIT_PRE_REBASE_UPSTREAM", upstream.clone());
    }
    if let Some(ref branch) = args.pre_rebase_branch {
        env.insert("PRE_COMMIT_PRE_REBASE_BRANCH", branch.clone());
    }
    if let Some(ref branch) = args.local_branch {
        env.insert("PRE_COMMIT_LOCAL_BRANCH", branch.clone());
    }
    if let Some(ref branch) = args.remote_branch {
        env.insert("PRE_COMMIT_REMOTE_BRANCH", branch.clone());
    }
    if let Some(ref name) = args.remote_name {
        env.insert("PRE_COMMIT_REMOTE_NAME", name.clone());
    }
    if let Some(ref url) = args.remote_url {
        env.insert("PRE_COMMIT_REMOTE_URL", url.clone());
    }
    if let Some(ref checkout) = args.checkout_type {
        env.insert("PRE_COMMIT_CHECKOUT_TYPE", checkout.clone());
    }
    if args.is_squash_merge {
        env.insert("PRE_COMMIT_SQUASH_MERGE", "1".into());
    }
    if let Some(ref command) = args.rewrite_command {
        env.insert("PRE_COMMIT_REWRITE_COMMAND", command.clone());
    }

    env
}

fn get_skips() -> Vec<String> {
    match std::env::var_os("SKIP") {
        Some(s) if !s.is_empty() => s
            .to_string_lossy()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>(),
        _ => vec![],
    }
}

async fn install_hook(hook: &Hook, env_dir: PathBuf) -> Result<()> {
    debug!(%hook, target = %env_dir.display(), "Install environment");

    if env_dir.try_exists()? {
        debug!(
            env_dir = %env_dir.display(),
            "Removing existing environment directory",
        );
        fs_err::remove_dir_all(&env_dir)?;
    }

    hook.language.install(hook).await?;
    hook.mark_installed()?;

    Ok(())
}

pub async fn install_hooks(hooks: &[Hook], reporter: &HookInstallReporter) -> Result<()> {
    let to_install = hooks
        .iter()
        .filter(|&hook| !hook.installed())
        .unique_by(|&hook| hook.install_key());

    let mut tasks = FuturesUnordered::new();
    for hook in to_install {
        if let Some(env_dir) = hook.environment_dir() {
            tasks.push(async move {
                let progress = reporter.on_install_start(hook);
                let result = install_hook(hook, env_dir).await;
                (result, progress)
            });
        }
    }
    while let Some((result, progress)) = tasks.next().await {
        reporter.on_install_complete(progress);
        result?;
    }

    reporter.on_complete();

    Ok(())
}

const SKIPPED: &str = "Skipped";
const NO_FILES: &str = "(no files to check)";

fn status_line(start: &str, cols: usize, end_msg: &str, end_color: Style, postfix: &str) -> String {
    let dots = cols - start.width_cjk() - end_msg.len() - postfix.len() - 1;
    format!(
        "{}{}{}{}",
        start,
        ".".repeat(dots),
        postfix,
        end_msg.style(end_color)
    )
}

fn calculate_columns(hooks: &[Hook]) -> usize {
    let name_len = hooks
        .iter()
        .map(|hook| hook.name.width_cjk())
        .max()
        .unwrap_or(0);
    max(80, name_len + 3 + NO_FILES.len() + 1 + SKIPPED.len())
}

/// Run all hooks.
pub async fn run_hooks(
    hooks: &[Hook],
    skips: &[String],
    filter: &FileFilter<'_>,
    env_vars: HashMap<&'static str, String>,
    fail_fast: bool,
    show_diff_on_failure: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let env_vars = Arc::new(env_vars);

    let columns = calculate_columns(hooks);
    let mut success = true;

    let mut diff = get_diff().await?;
    // hooks must run in serial
    for hook in hooks {
        let (hook_success, new_diff) = run_hook(
            hook,
            filter,
            env_vars.clone(),
            skips,
            diff,
            columns,
            verbose,
            printer,
        )
        .await?;

        success &= hook_success;
        diff = new_diff;
        if !success && (fail_fast || hook.fail_fast) {
            break;
        }
    }

    if !success && show_diff_on_failure {
        writeln!(printer.stdout(), "All changes made by hooks:")?;
        let color = match ColorChoice::global() {
            ColorChoice::Auto => "--color=auto",
            ColorChoice::Always | ColorChoice::AlwaysAnsi => "--color=always",
            ColorChoice::Never => "--color=never",
        };
        git_cmd("git diff")?
            .arg("--no-pager")
            .arg("diff")
            .arg("--no-ext-diff")
            .arg(color)
            .check(true)
            .spawn()?
            .wait()
            .await?;
    };

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
    hook: &Hook,
    filter: &FileFilter<'_>,
    env_vars: Arc<HashMap<&'static str, String>>,
    skips: &[String],
    diff: Vec<u8>,
    columns: usize,
    verbose: bool,
    printer: Printer,
) -> Result<(bool, Vec<u8>)> {
    if skips.contains(&hook.id) || skips.contains(&hook.alias) {
        writeln!(
            printer.stdout(),
            "{}",
            status_line(
                &hook.name,
                columns,
                SKIPPED,
                Style::new().black().on_yellow(),
                "",
            )
        )?;
        return Ok((true, diff));
    }

    let mut filenames = filter.for_hook(hook)?;

    if filenames.is_empty() && !hook.always_run {
        writeln!(
            printer.stdout(),
            "{}",
            status_line(
                &hook.name,
                columns,
                SKIPPED,
                Style::new().black().on_cyan(),
                NO_FILES,
            )
        )?;
        return Ok((true, diff));
    }

    write!(
        printer.stdout(),
        "{}{}",
        &hook.name,
        ".".repeat(columns - hook.name.width_cjk() - 6 - 1)
    )?;
    std::io::stdout().flush()?;

    let start = std::time::Instant::now();

    let (status, output) = if hook.pass_filenames {
        shuffle(&mut filenames);
        hook.language.run(hook, &filenames, env_vars).await?
    } else {
        hook.language.run(hook, &[], env_vars).await?
    };

    let duration = start.elapsed();

    let new_diff = get_diff().await?;
    let file_modified = diff != new_diff;
    let success = status == 0 && !file_modified;

    if success {
        writeln!(printer.stdout(), "{}", "Passed".on_green())?;
    } else {
        writeln!(printer.stdout(), "{}", "Failed".on_red())?;
    }

    if verbose || hook.verbose || !success {
        writeln!(
            printer.stdout(),
            "{}",
            format!("- hook id: {}", hook.id).dimmed()
        )?;
        if verbose || hook.verbose {
            writeln!(
                printer.stdout(),
                "{}",
                format!("- duration: {:.2?}s", duration.as_secs_f64()).dimmed()
            )?;
        }
        if status != 0 {
            writeln!(
                printer.stdout(),
                "{}",
                format!("- exit code: {status}").dimmed()
            )?;
        }
        if file_modified {
            writeln!(
                printer.stdout(),
                "{}",
                "- files were modified by this hook".dimmed()
            )?;
        }

        // To be consistent with pre-commit, merge stderr into stdout.
        let stdout = output.trim_ascii();
        if !stdout.is_empty() {
            if let Some(file) = hook.log_file.as_deref() {
                fs_err::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(file)
                    .and_then(|mut f| {
                        f.write_all(stdout)?;
                        Ok(())
                    })?;
            } else {
                writeln!(
                    printer.stdout(),
                    "{}",
                    textwrap::indent(&String::from_utf8_lossy(stdout), "  ").dimmed()
                )?;
            };
        }
    }

    Ok((success, new_diff))
}
