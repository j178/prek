use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result, ensure};
use clap::Parser;
use fancy_regex::Regex;
use memchr::memchr_iter;

use super::pattern::{regex_options, trim_line_ending};
use crate::hook::Hook;
use crate::hooks::{HookOutput, parse_hook_args, run_blocking_file_checks};

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Match the pattern case-insensitively.
    #[arg(short = 'i', long)]
    ignore_case: bool,
    /// Search each file as a whole.
    #[arg(short = 'm', long)]
    multiline: bool,
    /// Report files that need changes without modifying them.
    #[arg(long)]
    check: bool,
    /// Regular expression to replace.
    #[arg(value_name = "PATTERN")]
    pattern: String,
    /// Replacement template ($0, $1, ${name}, and $$ are supported).
    #[arg(value_name = "REPLACEMENT")]
    replacement: String,
}

enum ReplacementPart {
    Literal(Vec<u8>),
    Capture(usize),
}

// Resolve references before touching files. Expanding captures as bytes also
// preserves text files whose contents are not entirely valid UTF-8.
fn parse_replacement(mut template: &str, regex: &Regex) -> Result<Vec<ReplacementPart>> {
    let mut parts = Vec::new();
    let mut literal = Vec::new();

    while let Some(start) = template.find('$') {
        literal.extend_from_slice(&template.as_bytes()[..start]);
        template = &template[start + 1..];

        if let Some(rest) = template.strip_prefix('$') {
            literal.push(b'$');
            template = rest;
            continue;
        }

        let name;
        if let Some(rest) = template.strip_prefix('{') {
            let end = rest.find('}').context("Unclosed capture group reference")?;
            name = &rest[..end];
            ensure!(!name.is_empty(), "Empty capture group reference");
            template = &rest[end + 1..];
        } else {
            let end = template
                .bytes()
                .take_while(|byte| byte.is_ascii_alphanumeric() || *byte == b'_')
                .count();
            if end == 0 {
                literal.push(b'$');
                continue;
            }
            name = &template[..end];
            template = &template[end..];
        }

        let index = if let Ok(index) = name.parse::<usize>() {
            ensure!(
                index < regex.captures_len(),
                "Unknown capture group `{name}`"
            );
            index
        } else {
            regex
                .capture_names()
                .position(|candidate| candidate == Some(name))
                .with_context(|| format!("Unknown capture group `{name}`"))?
        };

        if !literal.is_empty() {
            parts.push(ReplacementPart::Literal(std::mem::take(&mut literal)));
        }
        parts.push(ReplacementPart::Capture(index));
    }

    literal.extend_from_slice(template.as_bytes());
    if !literal.is_empty() {
        parts.push(ReplacementPart::Literal(literal));
    }
    Ok(parts)
}

pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args = parse_hook_args::<Args>(hook)?;
    let regex = regex_options(args.ignore_case, args.multiline)
        .build(args.pattern.clone())
        .with_context(|| format!("Failed to compile regex pattern `{}`", args.pattern))?;
    let replacement = parse_replacement(&args.replacement, &regex)
        .with_context(|| format!("Invalid replacement template `{}`", args.replacement))?;

    run_blocking_file_checks(
        hook.project().relative_path(),
        &[],
        filenames,
        move |file_path, display_path| {
            fix_file(
                file_path,
                display_path,
                &regex,
                &replacement,
                args.multiline,
                args.check,
            )
        },
    )
    .await
}

fn fix_file(
    file_path: &Path,
    display_path: &Path,
    regex: &Regex,
    replacement: &[ReplacementPart],
    multiline: bool,
    check: bool,
) -> Result<HookOutput> {
    let contents = fs_err::read(file_path)?;
    let mut output = Vec::with_capacity(contents.len());
    let mut diagnostics = Vec::new();

    if multiline {
        let changed = replace(&contents, regex, replacement, &mut output).with_context(|| {
            format!("Failed to replace pattern in `{}`", display_path.display())
        })?;
        if check && let Some(changed) = changed {
            let line_number = memchr_iter(b'\n', &contents[..changed]).count() + 1;
            writeln!(
                diagnostics,
                "{}:{line_number}: matching text would be replaced",
                display_path.display()
            )?;
        }
    } else {
        for (index, line) in contents.split_inclusive(|&byte| byte == b'\n').enumerate() {
            let text = trim_line_ending(line);
            let start = output.len();
            replace(text, regex, replacement, &mut output).with_context(|| {
                format!(
                    "Failed to replace pattern in `{}:{}`",
                    display_path.display(),
                    index + 1
                )
            })?;
            if check && output[start..] != *text {
                writeln!(
                    diagnostics,
                    "{}:{}: matching text would be replaced",
                    display_path.display(),
                    index + 1
                )?;
            }
            output.extend_from_slice(&line[text.len()..]);
        }
    }

    if output == contents {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }
    if check {
        return Ok(HookOutput::unchanged(1, diagnostics));
    }

    // A later match can fail (for example, on the backtracking limit). Only
    // write once all matches in this file have been processed successfully.
    fs_err::write(file_path, output)?;
    Ok(HookOutput::known(
        1,
        format!("Fixing {}\n", display_path.display()).into_bytes(),
        true,
    ))
}

fn replace(
    contents: &[u8],
    regex: &Regex,
    replacement: &[ReplacementPart],
    output: &mut Vec<u8>,
) -> Result<Option<usize>> {
    let mut copied = 0;
    let mut first_changed = None;
    for captures in regex.captures_iter(contents) {
        let captures = captures?;
        let matched = captures.get(0).context("Missing full regex match")?;
        output.extend_from_slice(&contents[copied..matched.start()]);
        let start = output.len();
        for part in replacement {
            match part {
                ReplacementPart::Literal(literal) => output.extend_from_slice(literal),
                ReplacementPart::Capture(index) => {
                    if let Some(capture) = captures.get(*index) {
                        output.extend_from_slice(capture.as_bytes());
                    }
                }
            }
        }
        if first_changed.is_none() && output[start..] != *matched.as_bytes() {
            first_changed = Some(matched.start());
        }
        copied = matched.end();
    }
    output.extend_from_slice(&contents[copied..]);
    Ok(first_changed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// File transformations preserve untouched bytes and apply each original match once.
    #[test]
    fn replacement_preserves_file_contents() -> Result<()> {
        let cases = [
            (
                "(?P<first>foo)(?P<last>bar)?",
                r"${last}:$1:$$:$0:\n",
                false,
                b"\xef\xbb\xbffoobar\r\nfoo\xff".as_slice(),
                b"\xef\xbb\xbfbar:foo:$:foobar:\\n\r\n:foo:$:foo:\\n\xff".as_slice(),
            ),
            ("foo", "foobar", false, b"foo foo", b"foobar foobar"),
            ("foo", "", false, b"foo\r\nfoo", b"\r\n"),
            ("(?=é)", ">", false, "éé".as_bytes(), ">é>é".as_bytes()),
            ("^|$", "|", false, b"foo\r\n\nbar", b"|foo|\r\n|\n|bar|"),
            ("foo.*?bar", "x", false, b"foo\nbar\n", b"foo\nbar\n"),
            ("^foo.*?bar$", "x", true, b"foo\nbar\nfoo\nbar\n", b"x\nx\n"),
            ("foo", "one\ntwo", false, b"foo\r\n", b"one\ntwo\r\n"),
            ("^", "prefix", true, b"", b"prefix"),
        ];
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("file.txt");
        for (pattern, template, multiline, input, expected) in cases {
            fs_err::write(&path, input)?;
            let regex = regex_options(false, multiline).build(pattern.to_owned())?;
            let replacement = parse_replacement(template, &regex)?;
            fix_file(&path, &path, &regex, &replacement, multiline, false)?;
            assert_eq!(
                fs_err::read(&path)?,
                expected,
                "{pattern:?} -> {template:?}"
            );
        }
        Ok(())
    }
}
