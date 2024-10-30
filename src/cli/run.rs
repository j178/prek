use std::cmp::max;
use std::fmt::Write as _;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use anyhow::Result;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;
use owo_colors::{OwoColorize, Style};
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use regex::Regex;
use tokio::process::Command;
use tracing::{debug, trace};
use unicode_width::UnicodeWidthStr;

use crate::cli::ExitStatus;
use crate::config::Stage;
use crate::fs::normalize_path;
use crate::git::{get_all_files, get_changed_files, get_diff, get_staged_files};
use crate::hook::{Hook, Project};
use crate::identify::tags_from_path;
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
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let store = Store::from_settings()?.init()?;
    let mut project = Project::current(config)?;

    // TODO: check .pre-commit-config.yaml status and git status
    // TODO: fill env vars
    // TODO: impl staged_files_only

    let lock = store.lock_async().await?;
    let hooks = project.init_hooks(&store, printer).await?;

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
    let to_install = hooks
        .iter()
        .filter(|h| !skips.contains(&h.id) && !skips.contains(&h.alias))
        .cloned()
        .collect::<Vec<_>>();

    debug!(
        "Hooks going to run: {:?}",
        to_install.iter().map(|h| &h.id).collect::<Vec<_>>()
    );
    install_hooks(&to_install, printer).await?;
    drop(lock);

    let mut filenames = all_filenames(hook_stage, from_ref, to_ref, all_files, files).await?;
    for filename in &mut filenames {
        normalize_path(filename);
    }

    let filenames = filter_filenames(
        filenames.par_iter(),
        project.config().files.as_deref(),
        project.config().exclude.as_deref(),
    )?
    .collect();

    run_hooks(
        &hooks,
        &skips,
        filenames,
        project.config().fail_fast.unwrap_or(false),
        show_diff_on_failure,
        verbose,
        printer,
    )
    .await?;

    Ok(ExitStatus::Success)
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

/// Get all filenames to run hooks on.
#[allow(clippy::too_many_arguments)]
async fn all_filenames(
    hook_stage: Option<Stage>,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<PathBuf>,
) -> Result<Vec<String>> {
    if hook_stage.is_some_and(|stage| !stage.operate_on_files()) {
        return Ok(vec![]);
    }
    // if hook_stage.is_some_and(|stage| matches!(stage, Stage::PrepareCommitMsg | Stage::CommitMsg)) {
    //     return iter::once(commit_msg_filename.unwrap());
    // }
    if let (Some(from_ref), Some(to_ref)) = (from_ref, to_ref) {
        let files = get_changed_files(&from_ref, &to_ref).await?;
        debug!(
            "Files changed between {} and {}: {}",
            from_ref,
            to_ref,
            files.len()
        );
        return Ok(files);
    }

    if !files.is_empty() {
        let files: Vec<_> = files
            .into_iter()
            .map(|f| f.to_string_lossy().to_string())
            .collect();
        debug!("Files passed as arguments: {}", files.len());
        return Ok(files);
    }
    if all_files {
        let files = get_all_files().await?;
        debug!("All files in the repo: {}", files.len());
        return Ok(files);
    }
    // if is_in_merge_conflict() {
    //     return get_conflicted_files();
    // }
    let files = get_staged_files().await?;
    debug!("Staged files: {}", files.len());
    Ok(files)
}

async fn install_hook(hook: &Hook, env_dir: PathBuf, printer: Printer) -> Result<()> {
    writeln!(
        printer.stdout(),
        "Installing environment for {}",
        hook.repo(),
    )?;
    debug!("Install environment for {} to {}", hook, env_dir.display());

    if env_dir.try_exists()? {
        debug!(
            "Removing existing environment directory {}",
            env_dir.display()
        );
        fs_err::remove_dir_all(&env_dir)?;
    }

    hook.language.install(hook).await?;
    hook.mark_installed()?;

    Ok(())
}

// TODO: progress bar
async fn install_hooks(hooks: &[Hook], printer: Printer) -> Result<()> {
    let to_install = hooks
        .iter()
        .filter(|&hook| !hook.installed())
        .unique_by(|&hook| hook.install_key());

    let mut tasks = FuturesUnordered::new();
    for hook in to_install {
        if let Some(env_dir) = hook.environment_dir() {
            tasks.push(async move { install_hook(hook, env_dir, printer).await });
        }
    }
    while let Some(result) = tasks.next().await {
        result?;
    }

    Ok(())
}

struct FileTypeFilter {
    all: Vec<String>,
    any: Vec<String>,
    exclude: Vec<String>,
}

impl FileTypeFilter {
    fn new(types: &[String], types_or: &[String], exclude_types: &[String]) -> Self {
        let all = types.to_vec();
        let any = types_or.to_vec();
        let exclude = exclude_types.to_vec();
        Self { all, any, exclude }
    }

    fn filter(&self, file_types: &[&str]) -> bool {
        if !self.all.is_empty() && !self.all.iter().all(|t| file_types.contains(&t.as_str())) {
            return false;
        }
        if !self.any.is_empty() && !self.any.iter().any(|t| file_types.contains(&t.as_str())) {
            return false;
        }
        if self
            .exclude
            .iter()
            .any(|t| file_types.contains(&t.as_str()))
        {
            return false;
        }
        true
    }

    fn from_hook(hook: &Hook) -> Self {
        Self::new(&hook.types, &hook.types_or, &hook.exclude_types)
    }
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

async fn run_hooks(
    hooks: &[Hook],
    skips: &[String],
    filenames: Vec<&String>,
    fail_fast: bool,
    show_diff_on_failure: bool,
    verbose: bool,
    printer: Printer,
) -> Result<()> {
    let columns = calculate_columns(hooks);
    // TODO: progress bar, format output
    let mut success = true;

    let mut diff = get_diff().await?;
    // hooks must run in serial
    for hook in hooks {
        // TODO: handle single hook result
        let (hook_success, new_diff) =
            run_hook(hook, &filenames, skips, diff, columns, verbose, printer).await?;

        success &= hook_success;
        diff = new_diff;
        if !success && (fail_fast || hook.fail_fast) {
            break;
        }
    }

    if !success && show_diff_on_failure {
        writeln!(printer.stdout(), "All changes made by hooks:")?;
        // TODO: color
        Command::new("git")
            .arg("diff")
            .arg("--no-ext-diff")
            .arg("--no-pager")
            .spawn()?
            .wait()
            .await?;
    };

    Ok(())
}

async fn run_hook(
    hook: &Hook,
    filenames: &[&String],
    skips: &[String],
    diff: Vec<u8>,
    columns: usize,
    verbose: bool,
    printer: Printer,
) -> Result<(bool, Vec<u8>)> {
    // TODO: check files diff
    // TODO: group filenames and run in parallel, handle require_serial

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

    let filenames = filter_filenames(
        filenames.into_par_iter().copied(),
        hook.files.as_deref(),
        hook.exclude.as_deref(),
    )?;

    let filter = FileTypeFilter::from_hook(hook);
    let filenames: Vec<_> = filenames
        .filter(|&filename| {
            let path = Path::new(filename);
            match tags_from_path(path) {
                Ok(tags) => filter.filter(&tags),
                Err(err) => {
                    trace!("Failed to get tags for {filename}: {err}");
                    false
                }
            }
        })
        .collect();

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
    let start = std::time::Instant::now();

    let output = if hook.pass_filenames {
        hook.language.run(hook, &filenames).await?
    } else {
        hook.language.run(hook, &[]).await?
    };

    let duration = start.elapsed();

    let new_diff = get_diff().await?;
    let file_modified = diff != new_diff;
    let success = output.status.success() && !file_modified;

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
                format!("- duration: {:?}s", duration.as_secs()).dimmed()
            )?;
        }
        if !output.status.success() {
            writeln!(
                printer.stdout(),
                "{}",
                format!("- exit code: {}", output.status.code().unwrap_or_default()).dimmed()
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
        let stdout = output.stdout.trim_ascii();
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
                writeln!(printer.stdout(), "{}", String::from_utf8_lossy(stdout))?;
            };
        }
    }

    Ok((success, new_diff))
}

fn filter_filenames<'a>(
    filenames: impl ParallelIterator<Item = &'a String>,
    include: Option<&str>,
    exclude: Option<&str>,
) -> Result<impl ParallelIterator<Item = &'a String>, regex::Error> {
    let include = include.map(Regex::new).transpose()?;
    let exclude = exclude.map(Regex::new).transpose()?;

    Ok(filenames.filter(move |filename| {
        let filename = filename.as_str();
        if !include
            .as_ref()
            .map(|re| re.is_match(filename))
            .unwrap_or(true)
        {
            return false;
        }
        if exclude
            .as_ref()
            .map(|re| re.is_match(filename))
            .unwrap_or(false)
        {
            return false;
        }
        true
    }))
}
