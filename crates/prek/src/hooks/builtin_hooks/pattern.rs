use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use memchr::memchr_iter;
use regex_automata::{MatchKind, meta::Regex, util::syntax};

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Match patterns case-insensitively.
    #[arg(short = 'i', long)]
    ignore_case: bool,
    /// Search each file as a whole.
    #[arg(short = 'm', long)]
    multiline: bool,
    #[arg(required = true, value_name = "PATTERN")]
    patterns: Vec<String>,
}

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct FilenameArgs {
    /// Match patterns case-insensitively.
    #[arg(short = 'i', long)]
    ignore_case: bool,
    #[arg(required = true, value_name = "PATTERN")]
    patterns: Vec<String>,
}

#[derive(Clone, Copy)]
enum MatchPolicy {
    Deny,
    Require,
}

#[derive(Clone, Copy)]
enum ScanMode {
    Lines,
    Multiline,
}

struct Matcher {
    regex: Arc<Regex>,
    scan_mode: ScanMode,
}

impl Matcher {
    fn new(args: &Args) -> Result<Self> {
        let scan_mode = if args.multiline {
            ScanMode::Multiline
        } else {
            ScanMode::Lines
        };

        Ok(Self {
            regex: Arc::new(build_regex(
                &args.patterns,
                args.ignore_case,
                args.multiline,
            )?),
            scan_mode,
        })
    }
}

fn build_regex(patterns: &[String], ignore_case: bool, multiline: bool) -> Result<Regex> {
    let syntax = syntax::Config::new()
        // Enable case-insensitive matching for `-i` / `--ignore-case`.
        .case_insensitive(ignore_case)
        // Let `^` and `$` match line boundaries for `-m` / `--multiline`.
        .multi_line(multiline)
        // Let `.` match newlines for `-m` / `--multiline`.
        .dot_matches_new_line(multiline)
        // Compile byte-oriented patterns so arbitrary file bytes can match.
        .utf8(false);
    Regex::builder()
        .configure(
            Regex::config()
                // Return the earliest match, using pattern order to break ties.
                .match_kind(MatchKind::LeftmostFirst)
                // Allow empty matches at any byte offset, as byte regexes do.
                .utf8_empty(false),
        )
        .syntax(syntax)
        .build_many(patterns)
        .context("Failed to compile regex patterns")
}

pub(crate) async fn deny_pattern(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    run(hook, filenames, MatchPolicy::Deny).await
}

pub(crate) async fn require_pattern(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    run(hook, filenames, MatchPolicy::Require).await
}

pub(crate) fn deny_filename_pattern(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    run_filename_pattern(hook, filenames, MatchPolicy::Deny)
}

pub(crate) fn require_filename_pattern(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    run_filename_pattern(hook, filenames, MatchPolicy::Require)
}

fn run_filename_pattern(
    hook: &Hook,
    filenames: &[&Path],
    policy: MatchPolicy,
) -> Result<HookOutput> {
    let args =
        FilenameArgs::try_parse_from(hook.entry.expect_direct().split_with_args(&hook.args)?)?;
    let regex = build_regex(&args.patterns, args.ignore_case, false)?;
    let mut failed = false;
    let mut output = Vec::new();

    for filename in filenames {
        let matched = filename
            .file_name()
            .is_some_and(|basename| regex.is_match(basename.as_encoded_bytes()));
        let message = match policy {
            MatchPolicy::Deny if matched => "filename matches a denied pattern",
            MatchPolicy::Require if !matched => "filename does not match any required pattern",
            MatchPolicy::Deny | MatchPolicy::Require => continue,
        };

        failed = true;
        writeln!(output, "{}: {message}", filename.display())?;
    }

    Ok(HookOutput::unchanged(i32::from(failed), output))
}

async fn run(hook: &Hook, filenames: &[&Path], policy: MatchPolicy) -> Result<HookOutput> {
    let args = Args::try_parse_from(hook.entry.expect_direct().split_with_args(&hook.args)?)?;
    let matcher = Matcher::new(&args)?;
    let file_base = hook.project().relative_path();

    run_concurrent_file_checks(
        filenames.iter().copied(),
        *INTERNAL_CONCURRENCY,
        |filename| check_file(file_base, filename, &matcher, policy),
    )
    .await
}

async fn check_file(
    file_base: &Path,
    filename: &Path,
    matcher: &Matcher,
    policy: MatchPolicy,
) -> Result<HookOutput> {
    let (exit_status, output) = match matcher.scan_mode {
        ScanMode::Lines => check_lines(file_base, filename, &matcher.regex, policy).await,
        ScanMode::Multiline => check_multiline(file_base, filename, &matcher.regex, policy).await,
    }?;
    Ok(HookOutput::unchanged(exit_status, output))
}

async fn check_lines(
    file_base: &Path,
    filename: &Path,
    patterns: &Arc<Regex>,
    policy: MatchPolicy,
) -> Result<(i32, Vec<u8>)> {
    let file_path = file_base.join(filename);
    let filename = filename.to_path_buf();
    let patterns = Arc::clone(patterns);
    tokio::task::spawn_blocking(move || check_lines_sync(&file_path, &filename, &patterns, policy))
        .await?
}

fn check_lines_sync(
    file_path: &Path,
    filename: &Path,
    patterns: &Regex,
    policy: MatchPolicy,
) -> Result<(i32, Vec<u8>)> {
    let file = fs_err::File::open(file_path)?;
    let mut reader = BufReader::new(file);
    let mut matched = false;
    let mut output = Vec::new();
    let mut line = Vec::new();
    let mut line_number = 0;

    while reader.read_until(b'\n', &mut line)? != 0 {
        line_number += 1;
        let contents = trim_line_ending(&line);
        if patterns.is_match(contents) {
            if matches!(policy, MatchPolicy::Require) {
                return Ok((0, Vec::new()));
            }

            matched = true;
            write!(output, "{}:{line_number}:", filename.display())?;
            output.write_all(contents)?;
            writeln!(output)?;
        }
        line.clear();
    }

    match policy {
        MatchPolicy::Deny => Ok((i32::from(matched), output)),
        MatchPolicy::Require => Ok(missing_match(filename)),
    }
}

async fn check_multiline(
    file_base: &Path,
    filename: &Path,
    patterns: &Regex,
    policy: MatchPolicy,
) -> Result<(i32, Vec<u8>)> {
    let contents = fs_err::tokio::read(file_base.join(filename)).await?;
    match policy {
        MatchPolicy::Deny => {
            let Some(matched) = patterns.find(&contents) else {
                return Ok((0, Vec::new()));
            };
            let line_number = memchr_iter(b'\n', &contents[..matched.start()]).count() + 1;
            let matched = &contents[matched.range()];
            let mut output = Vec::new();
            write!(output, "{}:{line_number}:", filename.display())?;
            output.write_all(matched)?;
            if !matched.ends_with(b"\n") {
                writeln!(output)?;
            }
            Ok((1, output))
        }
        MatchPolicy::Require if patterns.is_match(&contents) => Ok((0, Vec::new())),
        MatchPolicy::Require => Ok(missing_match(filename)),
    }
}

fn missing_match(filename: &Path) -> (i32, Vec<u8>) {
    (
        1,
        format!("{}: no pattern matched\n", filename.display()).into_bytes(),
    )
}

fn trim_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}
