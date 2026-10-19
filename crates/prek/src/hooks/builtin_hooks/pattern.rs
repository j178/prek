use std::io::{BufRead, BufReader, Write};
use std::ops::Range;
use std::path::Path;

use anyhow::{Context, Result};
use clap::Parser;
use fancy_regex::{BytesMode, RegexInput, RegexOptionsBuilder, RegexSet};
use memchr::memchr_iter;

use crate::hook::Hook;
use crate::hooks::{HookOutput, parse_hook_args, run_blocking_file_checks};

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
    regex: RegexSet,
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
            regex: compile_patterns(&args.patterns, args.ignore_case, args.multiline)?,
            scan_mode,
        })
    }
}

fn compile_patterns(patterns: &[String], ignore_case: bool, multiline: bool) -> Result<RegexSet> {
    let mut options = RegexOptionsBuilder::new();
    options
        .case_insensitive(ignore_case)
        .multi_line(multiline)
        .dot_matches_new_line(multiline)
        .bytes_mode(BytesMode::UnicodeBytes);
    RegexSet::new_with_options(patterns, &options).with_context(|| {
        format!(
            "Failed to compile regex patterns `{}`",
            patterns.join("`, `")
        )
    })
}

fn find_first_match(patterns: &RegexSet, contents: &[u8]) -> Result<Option<Range<usize>>> {
    let Some(mut matches) = patterns.find_input(RegexInput::new(contents))? else {
        return Ok(None);
    };
    // The set returns the earliest matches in pattern order, preserving argument priority.
    Ok(matches
        .next()
        .transpose()?
        .map(|matched| matched.start()..matched.end()))
}

pub(crate) async fn deny_pattern(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    run_content_patterns(hook, filenames, MatchPolicy::Deny).await
}

pub(crate) async fn require_pattern(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    run_content_patterns(hook, filenames, MatchPolicy::Require).await
}

pub(crate) fn deny_filename_pattern(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    run_filename_patterns(hook, filenames, MatchPolicy::Deny)
}

pub(crate) fn require_filename_pattern(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    run_filename_patterns(hook, filenames, MatchPolicy::Require)
}

fn run_filename_patterns(
    hook: &Hook,
    filenames: &[&Path],
    policy: MatchPolicy,
) -> Result<HookOutput> {
    let args = parse_hook_args::<FilenameArgs>(hook)?;
    let regex = compile_patterns(&args.patterns, args.ignore_case, false)?;
    let mut failed = false;
    let mut output = Vec::new();

    for filename in filenames {
        let matched = if let Some(basename) = filename.file_name() {
            find_first_match(&regex, basename.as_encoded_bytes())
                .with_context(|| {
                    format!(
                        "Failed to match patterns against filename `{}`",
                        filename.display()
                    )
                })?
                .is_some()
        } else {
            false
        };
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

async fn run_content_patterns(
    hook: &Hook,
    filenames: &[&Path],
    policy: MatchPolicy,
) -> Result<HookOutput> {
    let args = parse_hook_args::<Args>(hook)?;
    let matcher = Matcher::new(&args)?;
    run_blocking_file_checks(
        hook.project().relative_path(),
        &[],
        filenames,
        move |file_path, display_path| check_file(file_path, display_path, &matcher, policy),
    )
    .await
}

fn check_file(
    file_path: &Path,
    display_path: &Path,
    matcher: &Matcher,
    policy: MatchPolicy,
) -> Result<HookOutput> {
    let (exit_status, output) = match matcher.scan_mode {
        ScanMode::Lines => check_file_lines(file_path, display_path, &matcher.regex, policy),
        ScanMode::Multiline => {
            check_file_multiline(file_path, display_path, &matcher.regex, policy)
        }
    }?;
    Ok(HookOutput::unchanged(exit_status, output))
}

fn check_file_lines(
    file_path: &Path,
    display_path: &Path,
    patterns: &RegexSet,
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
        if find_first_match(patterns, contents)
            .with_context(|| {
                format!(
                    "Failed to match patterns in `{}:{line_number}`",
                    display_path.display()
                )
            })?
            .is_some()
        {
            if matches!(policy, MatchPolicy::Require) {
                return Ok((0, Vec::new()));
            }

            matched = true;
            write!(output, "{}:{line_number}:", display_path.display())?;
            output.write_all(contents)?;
            writeln!(output)?;
        }
        line.clear();
    }

    match policy {
        MatchPolicy::Deny => Ok((i32::from(matched), output)),
        MatchPolicy::Require => Ok(missing_match(display_path)),
    }
}

fn check_file_multiline(
    file_path: &Path,
    display_path: &Path,
    patterns: &RegexSet,
    policy: MatchPolicy,
) -> Result<(i32, Vec<u8>)> {
    let contents = fs_err::read(file_path)?;
    let matched = find_first_match(patterns, &contents)
        .with_context(|| format!("Failed to match patterns in `{}`", display_path.display()))?;
    match policy {
        MatchPolicy::Deny => {
            let Some(matched) = matched else {
                return Ok((0, Vec::new()));
            };
            let line_number = memchr_iter(b'\n', &contents[..matched.start]).count() + 1;
            let matched = &contents[matched];
            let mut output = Vec::new();
            write!(output, "{}:{line_number}:", display_path.display())?;
            output.write_all(matched)?;
            if !matched.ends_with(b"\n") {
                writeln!(output)?;
            }
            Ok((1, output))
        }
        MatchPolicy::Require if matched.is_some() => Ok((0, Vec::new())),
        MatchPolicy::Require => Ok(missing_match(display_path)),
    }
}

fn missing_match(filename: &Path) -> (i32, Vec<u8>) {
    (
        1,
        format!(
            "{}: file does not match any required pattern\n",
            filename.display()
        )
        .into_bytes(),
    )
}

fn trim_line_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}
