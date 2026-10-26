use std::path::{Path, PathBuf};

use anyhow::Result;
use bstr::ByteSlice;
use clap::{Parser, ValueEnum};
use memchr::memchr2;

use crate::hook::Hook;
use crate::hooks::{HookOutput, parse_hook_args, run_blocking_file_checks};

const BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Indentation style to enforce.
    #[arg(long, value_enum, default_value_t = IndentStyle::Spaces)]
    indent: IndentStyle,
    /// Number of columns between tab stops (1-255).
    #[arg(long, default_value_t = 4, value_parser = clap::value_parser!(u8).range(1..))]
    tab_size: u8,
    /// Report files that need changes without modifying them.
    #[arg(long)]
    check: bool,
    #[arg(value_name = "FILENAMES")]
    filenames: Vec<PathBuf>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum IndentStyle {
    /// Indent with spaces only.
    Spaces,
    /// Indent with tabs, followed by spaces for any partial tab stop.
    Tabs,
}

/// Whether a file's indentation differs from the requested style.
///
/// In check mode `Changed` means the file would change, since nothing is written.
enum Outcome {
    Unchanged,
    Changed,
}

/// Runs the `normalize-indentation` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;
    let (style, tab_size, check) = (args.indent, usize::from(args.tab_size), args.check);

    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        move |file_path, display_path| fix_file(file_path, display_path, style, tab_size, check),
    )
    .await
}

fn fix_file(
    file_path: &Path,
    display_path: &Path,
    style: IndentStyle,
    tab_size: usize,
    check: bool,
) -> Result<HookOutput> {
    match (process_file(file_path, style, tab_size, check)?, check) {
        (Outcome::Unchanged, _) => Ok(HookOutput::unchanged(0, Vec::new())),
        (Outcome::Changed, true) => Ok(HookOutput::unchanged(
            1,
            format!(
                "{}: indentation needs normalization\n",
                display_path.display()
            )
            .into_bytes(),
        )),
        (Outcome::Changed, false) => Ok(HookOutput::known(
            1,
            format!("Fixing {}\n", display_path.display()).into_bytes(),
            true,
        )),
    }
}

/// Normalizes the file, or only checks it when `check` is set.
///
/// The file is processed one chunk at a time, and check mode stops at the first chunk that
/// would change.
fn process_file(path: &Path, style: IndentStyle, tab_size: usize, check: bool) -> Result<Outcome> {
    let contents = fs_err::read(path)?;

    // Binary files can be passed explicitly, bypassing the text file filter.
    if contents.find_byte(0).is_some() {
        return Ok(Outcome::Unchanged);
    }

    let bom_len = if contents.starts_with(BOM) {
        BOM.len()
    } else {
        0
    };
    let mut output = contents[..bom_len].to_vec();
    let mut changed = false;

    for chunk in line_chunks(&contents[bom_len..]) {
        let rewritten = reindent(chunk, style, tab_size);
        if check {
            if rewritten.is_some() {
                return Ok(Outcome::Changed);
            }
        } else {
            changed |= rewritten.is_some();
            output.extend_from_slice(rewritten.as_deref().unwrap_or(chunk));
        }
    }

    if !changed {
        return Ok(Outcome::Unchanged);
    }
    fs_err::write(path, &output)?;
    Ok(Outcome::Changed)
}

const fn is_indent(byte: u8) -> bool {
    matches!(byte, b' ' | b'\t')
}

const fn is_terminator(byte: u8) -> bool {
    matches!(byte, b'\r' | b'\n')
}

/// Splits `bytes` after its longest prefix of bytes that satisfy `predicate`.
fn split_prefix(bytes: &[u8], predicate: fn(u8) -> bool) -> (&[u8], &[u8]) {
    let len = bytes.iter().take_while(|byte| predicate(**byte)).count();
    bytes.split_at(len)
}

/// Splits `contents` into chunks that each start at the beginning of a line.
///
/// A chunk is the text of one line followed by its terminators, including any directly
/// following empty lines. Only the start of a chunk can hold indentation to rewrite, since
/// the empty lines that follow it have none.
fn line_chunks(contents: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut rest = contents;
    std::iter::from_fn(move || {
        if rest.is_empty() {
            return None;
        }

        let text_len = memchr2(b'\r', b'\n', rest).unwrap_or(rest.len());
        let (terminators, _) = split_prefix(&rest[text_len..], is_terminator);
        let (chunk, tail) = rest.split_at(text_len + terminators.len());
        rest = tail;
        Some(chunk)
    })
}

/// Returns `chunk` with normalized indentation, or `None` if it is already normalized.
fn reindent(chunk: &[u8], style: IndentStyle, tab_size: usize) -> Option<Vec<u8>> {
    let (indent, rest) = split_prefix(chunk, is_indent);

    // Blank and whitespace-only lines are left to the trailing whitespace hook.
    if rest.first().is_none_or(|byte| is_terminator(*byte)) {
        return None;
    }

    let canonical = canonical_indent(indent_columns(indent, tab_size), style, tab_size);
    if indent.iter().copied().eq(canonical.clone()) {
        return None;
    }
    Some(canonical.chain(rest.iter().copied()).collect())
}

fn canonical_indent(
    columns: usize,
    style: IndentStyle,
    tab_size: usize,
) -> impl Iterator<Item = u8> + Clone {
    let (tabs, spaces) = match style {
        IndentStyle::Spaces => (0, columns),
        IndentStyle::Tabs => (columns / tab_size, columns % tab_size),
    };
    std::iter::repeat_n(b'\t', tabs).chain(std::iter::repeat_n(b' ', spaces))
}

/// Width of `indent` in columns, where a tab advances to the next multiple of `tab_size`.
fn indent_columns(indent: &[u8], tab_size: usize) -> usize {
    indent.iter().fold(0, |column, byte| {
        if *byte == b'\t' {
            column + tab_size - column % tab_size
        } else {
            column + 1
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn convert(input: &[u8], style: IndentStyle, tab_size: usize) -> Result<Vec<u8>> {
        let dir = tempdir()?;
        let path = dir.path().join("file.txt");
        fs_err::write(&path, input)?;
        process_file(&path, style, tab_size, false)?;
        Ok(fs_err::read(&path)?)
    }

    fn spaces(input: &[u8], tab_size: usize) -> Result<Vec<u8>> {
        convert(input, IndentStyle::Spaces, tab_size)
    }

    fn tabs(input: &[u8], tab_size: usize) -> Result<Vec<u8>> {
        convert(input, IndentStyle::Tabs, tab_size)
    }

    fn parse_args(flags: &[&str]) -> Args {
        Args::parse_from(
            ["normalize-indentation"]
                .into_iter()
                .chain(flags.iter().copied()),
        )
    }

    #[test]
    fn tabs_become_spaces() -> Result<()> {
        assert_eq!(spaces(b"\tfoo\n\t\tbar\n", 4)?, b"    foo\n        bar\n");
        assert_eq!(spaces(b"\tfoo\n", 2)?, b"  foo\n");
        Ok(())
    }

    #[test]
    fn tab_advances_to_next_tab_stop() -> Result<()> {
        assert_eq!(spaces(b"  \tfoo\n", 4)?, b"    foo\n");
        assert_eq!(spaces(b"     \tfoo\n", 4)?, b"        foo\n");
        assert_eq!(spaces(b" \t \tfoo\n", 4)?, b"        foo\n");
        Ok(())
    }

    #[test]
    fn spaces_become_tabs_with_remainder() -> Result<()> {
        assert_eq!(tabs(b"        foo\n", 4)?, b"\t\tfoo\n");
        assert_eq!(tabs(b"      foo\n", 4)?, b"\t  foo\n");
        assert_eq!(tabs(b"  foo\n", 4)?, b"  foo\n");
        assert_eq!(tabs(b"   foo\n", 2)?, b"\t foo\n");
        Ok(())
    }

    #[test]
    fn mixed_indentation_is_canonicalized() -> Result<()> {
        assert_eq!(tabs(b"  \t  foo\n", 4)?, b"\t  foo\n");
        assert_eq!(spaces(b"\t  \tfoo\n", 4)?, b"        foo\n");
        Ok(())
    }

    #[test]
    fn only_leading_whitespace_changes() -> Result<()> {
        assert_eq!(spaces(b"\ta\tb  c\t\n", 4)?, b"    a\tb  c\t\n");
        assert_eq!(tabs(b"    a    b\n", 4)?, b"\ta    b\n");
        Ok(())
    }

    #[test]
    fn whitespace_only_lines_are_untouched() -> Result<()> {
        assert_eq!(spaces(b"\t\n  \t \n\n\tx\n", 4)?, b"\t\n  \t \n\n    x\n");
        assert_eq!(spaces(b"\t", 4)?, b"\t");
        assert_eq!(spaces(b"a\n\n\n\tb", 4)?, b"a\n\n\n    b");
        Ok(())
    }

    #[test]
    fn line_endings_are_preserved() -> Result<()> {
        assert_eq!(
            spaces(b"\ta\r\n\tb\n\tc\r\td", 4)?,
            b"    a\r\n    b\n    c\r    d"
        );
        assert_eq!(spaces(b"\t\r\n\t", 4)?, b"\t\r\n\t");
        assert_eq!(spaces(b"a\r\n\r\n\tb\r\n", 4)?, b"a\r\n\r\n    b\r\n");
        Ok(())
    }

    #[test]
    fn missing_final_newline_is_preserved() -> Result<()> {
        assert_eq!(spaces(b"\tfoo", 4)?, b"    foo");
        assert_eq!(spaces(b"foo\n", 4)?, b"foo\n");
        Ok(())
    }

    #[test]
    fn byte_order_marker_is_preserved() -> Result<()> {
        assert_eq!(
            spaces(b"\xEF\xBB\xBF\tfoo\n\tbar\n", 4)?,
            b"\xEF\xBB\xBF    foo\n    bar\n"
        );
        assert_eq!(
            spaces(b"\xEF\xBB\xBFok\n\tbar\n", 4)?,
            b"\xEF\xBB\xBFok\n    bar\n"
        );
        assert_eq!(spaces(b"\xEF\xBB\xBF", 4)?, b"\xEF\xBB\xBF");
        Ok(())
    }

    #[test]
    fn invalid_utf8_content_is_preserved() -> Result<()> {
        assert_eq!(spaces(b"\t\xFF\xFE\n", 4)?, b"    \xFF\xFE\n");
        Ok(())
    }

    #[test]
    fn empty_input_is_unchanged() -> Result<()> {
        assert_eq!(spaces(b"", 4)?, b"");
        assert_eq!(tabs(b"", 4)?, b"");
        Ok(())
    }

    #[test]
    fn normalization_is_idempotent() -> Result<()> {
        let input = b"\t  foo\n   \tbar\n      baz\n";
        for tab_size in [1, 2, 4, 8] {
            let once = spaces(input, tab_size)?;
            assert_eq!(spaces(&once, tab_size)?, once);
            let once = tabs(input, tab_size)?;
            assert_eq!(tabs(&once, tab_size)?, once);
        }
        Ok(())
    }

    #[test]
    fn args_default_to_spaces_and_tab_size_four() {
        let args = parse_args(&[]);
        assert!(matches!(args.indent, IndentStyle::Spaces));
        assert_eq!(args.tab_size, 4);
        assert!(!args.check);
    }

    #[test]
    fn args_reject_invalid_values() {
        for value in ["0", "256", "-1", "x"] {
            assert!(Args::try_parse_from(["normalize-indentation", "--tab-size", value]).is_err());
        }
        assert!(Args::try_parse_from(["normalize-indentation", "--indent=mixed"]).is_err());
    }

    fn run_on(content: &[u8], flags: &[&str]) -> Result<(HookOutput, Vec<u8>)> {
        let args = parse_args(flags);
        let tab_size = usize::from(args.tab_size);
        let dir = tempdir()?;
        let path = dir.path().join("file.txt");
        fs_err::write(&path, content)?;
        let output = fix_file(&path, &path, args.indent, tab_size, args.check)?;
        Ok((output, fs_err::read(&path)?))
    }

    #[test]
    fn fix_rewrites_file() -> Result<()> {
        let (output, contents) = run_on(b"\tfoo\n", &[])?;
        assert_eq!(output.exit_status, 1);
        assert!(output.output.starts_with(b"Fixing "));
        assert_eq!(contents, b"    foo\n");
        Ok(())
    }

    #[test]
    fn check_reports_without_writing() -> Result<()> {
        let (output, contents) = run_on(b"\tfoo\n", &["--check"])?;
        assert_eq!(output.exit_status, 1);
        assert!(
            output
                .output
                .ends_with(b": indentation needs normalization\n")
        );
        assert_eq!(contents, b"\tfoo\n");
        Ok(())
    }

    #[test]
    fn normalized_file_passes() -> Result<()> {
        let (output, contents) = run_on(b"    foo\n", &["--check"])?;
        assert_eq!(output.exit_status, 0);
        assert_eq!(output.output, b"");
        assert_eq!(contents, b"    foo\n");
        Ok(())
    }

    #[test]
    fn binary_file_is_skipped() -> Result<()> {
        let (output, contents) = run_on(b"\t\0foo\n", &[])?;
        assert_eq!(output.exit_status, 0);
        assert_eq!(contents, b"\t\0foo\n");
        Ok(())
    }
}
