use std::cmp::max;
use std::collections::HashMap;
use std::fmt::Write as _;
use std::future::Future;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;

use anstream::ColorChoice;
use anyhow::Result;
use fancy_regex::{self as regex, Regex};
use owo_colors::{OwoColorize, Style};
use rand::prelude::{SliceRandom, StdRng};
use rand::SeedableRng;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use tokio::process::Command;
use tokio::task::JoinSet;
use tracing::{error, trace};
use unicode_width::UnicodeWidthStr;

use crate::cli::ExitStatus;
use crate::git::get_diff;
use crate::hook::Hook;
use crate::identify::tags_from_path;
use crate::printer::Printer;

const SKIPPED: &str = "Skipped";
const NO_FILES: &str = "(no files to check)";

/// Filter filenames by include/exclude patterns.
pub struct FilenameFilter {
    include: Option<Regex>,
    exclude: Option<Regex>,
}

impl FilenameFilter {
    pub fn new(include: Option<&str>, exclude: Option<&str>) -> Result<Self, Box<regex::Error>> {
        let include = include.map(Regex::new).transpose()?;
        let exclude = exclude.map(Regex::new).transpose()?;
        Ok(Self { include, exclude })
    }

    pub fn filter(&self, filename: impl AsRef<str>) -> bool {
        let filename = filename.as_ref();
        if let Some(re) = &self.include {
            if !re.is_match(filename).unwrap_or(false) {
                return false;
            }
        }
        if let Some(re) = &self.exclude {
            if re.is_match(filename).unwrap_or(false) {
                return false;
            }
        }
        true
    }

    pub fn from_hook(hook: &Hook) -> Result<Self, Box<regex::Error>> {
        Self::new(hook.files.as_deref(), hook.exclude.as_deref())
    }
}

/// Filter files by tags.
struct FileTagFilter<'a> {
    all: &'a [String],
    any: &'a [String],
    exclude: &'a [String],
}

impl<'a> FileTagFilter<'a> {
    fn new(types: &'a [String], types_or: &'a [String], exclude_types: &'a [String]) -> Self {
        Self {
            all: types,
            any: types_or,
            exclude: exclude_types,
        }
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

    fn from_hook(hook: &'a Hook) -> Self {
        Self::new(&hook.types, &hook.types_or, &hook.exclude_types)
    }
}

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
    filenames: Vec<String>,
    env_vars: HashMap<&'static str, String>,
    fail_fast: bool,
    show_diff_on_failure: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let env_vars = Arc::new(env_vars);

    let columns = calculate_columns(hooks);
    // TODO: progress bar, format output
    let mut success = true;

    let mut diff = get_diff().await?;
    // hooks must run in serial
    for hook in hooks {
        let (hook_success, new_diff) = run_hook(
            hook,
            &filenames,
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
        Command::new("git")
            .arg("--no-pager")
            .arg("diff")
            .arg("--no-ext-diff")
            .arg(color)
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
    filenames: &[String],
    env_vars: Arc<HashMap<&'static str, String>>,
    skips: &[String],
    diff: Vec<u8>,
    columns: usize,
    verbose: bool,
    printer: Printer,
) -> anyhow::Result<(bool, Vec<u8>)> {
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

    let filter = FilenameFilter::from_hook(hook)?;
    let filenames = filenames
        .into_par_iter()
        .filter(|filename| filter.filter(filename));

    let filter = FileTagFilter::from_hook(hook);
    let mut filenames: Vec<_> = filenames
        .filter(|filename| {
            let path = Path::new(filename);
            match tags_from_path(path) {
                Ok(tags) => filter.filter(&tags),
                Err(err) => {
                    error!(filename, error = %err, "Failed to get tags");
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
                writeln!(printer.stdout(), "{}", String::from_utf8_lossy(stdout))?;
            };
        }
    }

    Ok((success, new_diff))
}

fn target_concurrency(serial: bool) -> usize {
    if serial || std::env::var_os("PRE_COMMIT_NO_CONCURRENCY").is_some() {
        1
    } else {
        std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1)
    }
}

// TODO: do a more accurate calculation
fn partitions<'a>(
    hook: &'a Hook,
    filenames: &'a [&String],
    concurrency: usize,
) -> Vec<Vec<&'a String>> {
    // If there are no filenames, we still want to run the hook once.
    if filenames.is_empty() {
        return vec![vec![]];
    }

    let max_per_batch = max(4, filenames.len().div_ceil(concurrency));
    // TODO: subtract the env size
    let max_cli_length = if cfg!(unix) {
        1 << 12
    } else {
        (1 << 15) - 2048 // UNICODE_STRING max - headroom
    };

    let command_length =
        hook.entry.len() + hook.args.iter().map(String::len).sum::<usize>() + hook.args.len();

    let mut partitions = Vec::new();
    let mut current = Vec::new();
    let mut current_length = command_length + 1;

    for &filename in filenames {
        let length = filename.len() + 1;
        if current_length + length > max_cli_length || current.len() >= max_per_batch {
            partitions.push(current);
            current = Vec::new();
            current_length = command_length + 1;
        }
        current.push(filename);
        current_length += length;
    }

    if !current.is_empty() {
        partitions.push(current);
    }

    partitions
}

pub async fn run_by_batch<T, F, Fut>(
    hook: &Hook,
    filenames: &[&String],
    run: F,
) -> anyhow::Result<Vec<T>>
where
    F: Fn(Vec<String>) -> Fut,
    F: Clone + Send + Sync + 'static,
    Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
    T: Send + 'static,
{
    let mut concurrency = target_concurrency(hook.require_serial);

    // Split files into batches
    let partitions = partitions(hook, filenames, concurrency);
    concurrency = concurrency.min(partitions.len());
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    trace!(
        total_files = filenames.len(),
        partitions = partitions.len(),
        concurrency = concurrency,
        "Running {}",
        hook.id,
    );

    let run = Arc::new(run);

    // Spawn tasks for each batch
    let mut tasks = JoinSet::new();

    for batch in partitions {
        let semaphore = semaphore.clone();
        let run = run.clone();

        let batch: Vec<_> = batch.into_iter().map(ToString::to_string).collect();

        tasks.spawn(async move {
            let _permit = semaphore
                .acquire()
                .await
                .map_err(|_| anyhow::anyhow!("Failed to acquire semaphore"))?;

            run(batch).await
        });
    }

    let mut results = Vec::new();
    while let Some(result) = tasks.join_next().await {
        results.push(result??);
    }

    Ok(results)
}
