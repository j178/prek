use std::path::{Path, PathBuf};

use anyhow::Result;
use bstr::ByteSlice;
use clap::Parser;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{contents_equal, parse_hook_args, run_blocking_file_checks};

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Report files that would change without modifying them.
    #[arg(long)]
    check: bool,
    /// Sort lines case-insensitively.
    #[arg(long, conflicts_with = "unique")]
    ignore_case: bool,
    /// Remove duplicate lines.
    #[arg(long, conflicts_with = "ignore_case")]
    unique: bool,
    #[arg(value_name = "FILENAMES")]
    filenames: Vec<PathBuf>,
}

/// Runs the `file-contents-sorter` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;

    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        move |file_path, display_path| {
            sort_file(
                file_path,
                display_path,
                args.check,
                args.ignore_case,
                args.unique,
            )
        },
    )
    .await
}

fn sort_file(
    file_path: &Path,
    display_path: &Path,
    check: bool,
    ignore_case: bool,
    unique: bool,
) -> Result<HookOutput> {
    let before = fs_err::read(file_path)?;
    let lines = sorted_lines(&before, ignore_case, unique);

    if contents_equal(&before, lines.iter().flat_map(|&line| [line, b"\n"])) {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    if check {
        return Ok(HookOutput::unchanged(
            1,
            format!("Would sort {}\n", display_path.display()).into_bytes(),
        ));
    }

    let mut after =
        Vec::with_capacity(lines.iter().map(|line| line.len()).sum::<usize>() + lines.len());
    for line in lines {
        after.extend_from_slice(line);
        after.push(b'\n');
    }
    fs_err::write(file_path, &after)?;
    Ok(HookOutput::known(
        1,
        format!("Sorting {}\n", display_path.display()).into_bytes(),
        true,
    ))
}

fn sorted_lines(before: &[u8], ignore_case: bool, unique: bool) -> Vec<&[u8]> {
    let mut lines = before
        .lines_with_terminator()
        .filter_map(normalize_line)
        .collect::<Vec<_>>();

    if ignore_case {
        lines.sort_by_cached_key(|line| line.to_ascii_lowercase());
    } else {
        lines.sort_unstable();
        if unique {
            lines.dedup();
        }
    }

    lines
}

fn normalize_line(mut line: &[u8]) -> Option<&[u8]> {
    line = line.trim_end_with(|byte| matches!(byte, '\n' | '\r'));

    // Drop empty and whitespace-only lines.
    if line.trim_ascii().is_empty() {
        None
    } else {
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &[u8],
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[test]
    fn test_sorted_lines_sorts_and_drops_blank_lines() {
        let before = b"beta\n\n  \nalpha\r\n";
        let after = sorted_lines(before, false, false);
        assert_eq!(after, [b"alpha".as_slice(), b"beta"]);
    }

    #[test]
    fn test_sorted_lines_ignore_case() {
        let before = b"Banana\napple\nApricot\n";
        let after = sorted_lines(before, true, false);
        assert_eq!(after, [b"apple".as_slice(), b"Apricot", b"Banana"]);
    }

    #[test]
    fn test_sorted_lines_ignore_case_is_stable_for_equal_keys() {
        let before = b"Apple\napple\n";
        let after = sorted_lines(before, true, false);
        assert_eq!(after, [b"Apple".as_slice(), b"apple"]);
    }

    #[test]
    fn test_sorted_lines_unique() {
        let before = b"beta\nalpha\nbeta\n";
        let after = sorted_lines(before, false, true);
        assert_eq!(after, [b"alpha".as_slice(), b"beta"]);
    }

    #[tokio::test]
    async fn test_sort_file_modifies_unsorted_file() -> Result<()> {
        let dir = tempdir()?;
        let relative = PathBuf::from("allowlist.txt");
        let file_path = create_test_file(&dir, "allowlist.txt", b"beta\nalpha\n").await?;

        let args = Args::try_parse_from(["file-contents-sorter"])?;
        let result = sort_file(
            &file_path,
            &relative,
            args.check,
            args.ignore_case,
            args.unique,
        )?;

        assert_eq!(result.exit_status, 1);
        assert_eq!(String::from_utf8(result.output)?, "Sorting allowlist.txt\n");
        assert_eq!(fs_err::tokio::read(&file_path).await?, b"alpha\nbeta\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_sort_file_keeps_sorted_file() -> Result<()> {
        let dir = tempdir()?;
        let relative = PathBuf::from("allowlist.txt");
        let file_path = create_test_file(&dir, "allowlist.txt", b"alpha\nbeta\n").await?;

        let args = Args::try_parse_from(["file-contents-sorter"])?;
        let result = sort_file(
            &file_path,
            &relative,
            args.check,
            args.ignore_case,
            args.unique,
        )?;

        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        assert_eq!(fs_err::tokio::read(&file_path).await?, b"alpha\nbeta\n");

        Ok(())
    }
}
