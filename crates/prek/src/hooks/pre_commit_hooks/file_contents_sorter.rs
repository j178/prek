use std::path::Path;

use anyhow::Result;
use bstr::ByteSlice;
use clap::Parser;

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::run::CONCURRENCY;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long, conflicts_with = "unique")]
    ignore_case: bool,
    #[arg(long, conflicts_with = "ignore_case")]
    unique: bool,
}

pub(crate) async fn file_contents_sorter(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.expect_direct().split()?.iter().chain(&hook.args))?;
    let file_base = hook.project().relative_path();

    run_concurrent_file_checks(filenames.iter().copied(), *CONCURRENCY, |filename| {
        sort_file(file_base, filename, args.ignore_case, args.unique)
    })
    .await
}

async fn sort_file(
    file_base: &Path,
    filename: &Path,
    ignore_case: bool,
    unique: bool,
) -> Result<(i32, Vec<u8>)> {
    let file_path = file_base.join(filename);
    let before = fs_err::tokio::read(&file_path).await?;
    let after = sorted_contents(&before, ignore_case, unique);

    if before == after {
        return Ok((0, Vec::new()));
    }

    fs_err::tokio::write(&file_path, &after).await?;
    Ok((1, format!("Sorting {}\n", filename.display()).into_bytes()))
}

fn sorted_contents(before: &[u8], ignore_case: bool, unique: bool) -> Vec<u8> {
    let mut lines = before
        .split_inclusive(|&byte| byte == b'\n')
        .filter_map(normalize_line)
        .collect::<Vec<_>>();

    if ignore_case {
        lines.sort_by(|left, right| cmp_ignore_ascii_case(left, right));
    } else {
        lines.sort_unstable();
        if unique {
            lines.dedup();
        }
    }

    if lines.is_empty() {
        return Vec::new();
    }

    let mut after =
        Vec::with_capacity(lines.iter().map(|line| line.len()).sum::<usize>() + lines.len());
    for line in lines {
        after.extend_from_slice(line);
        after.push(b'\n');
    }
    after
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

fn cmp_ignore_ascii_case(left: &[u8], right: &[u8]) -> std::cmp::Ordering {
    left.iter()
        .map(u8::to_ascii_lowercase)
        .cmp(right.iter().map(u8::to_ascii_lowercase))
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
    fn test_sorted_contents_sorts_and_drops_blank_lines() {
        let before = b"beta\n\n  \nalpha\r\n";
        let after = sorted_contents(before, false, false);
        assert_eq!(after, b"alpha\nbeta\n");
    }

    #[test]
    fn test_sorted_contents_ignore_case() {
        let before = b"Banana\napple\nApricot\n";
        let after = sorted_contents(before, true, false);
        assert_eq!(after, b"apple\nApricot\nBanana\n");
    }

    #[test]
    fn test_sorted_contents_ignore_case_is_stable_for_equal_keys() {
        let before = b"Apple\napple\n";
        let after = sorted_contents(before, true, false);
        assert_eq!(after, b"Apple\napple\n");
    }

    #[test]
    fn test_sorted_contents_unique() {
        let before = b"beta\nalpha\nbeta\n";
        let after = sorted_contents(before, false, true);
        assert_eq!(after, b"alpha\nbeta\n");
    }

    #[tokio::test]
    async fn test_sort_file_modifies_unsorted_file() -> Result<()> {
        let dir = tempdir()?;
        let relative = PathBuf::from("allowlist.txt");
        let file_path = create_test_file(&dir, "allowlist.txt", b"beta\nalpha\n").await?;

        let (code, output) = sort_file(dir.path(), &relative, false, false).await?;

        assert_eq!(code, 1);
        assert_eq!(String::from_utf8(output)?, "Sorting allowlist.txt\n");
        assert_eq!(fs_err::tokio::read(&file_path).await?, b"alpha\nbeta\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_sort_file_keeps_sorted_file() -> Result<()> {
        let dir = tempdir()?;
        let relative = PathBuf::from("allowlist.txt");
        let file_path = create_test_file(&dir, "allowlist.txt", b"alpha\nbeta\n").await?;

        let (code, output) = sort_file(dir.path(), &relative, false, false).await?;

        assert_eq!(code, 0);
        assert!(output.is_empty());
        assert_eq!(fs_err::tokio::read(&file_path).await?, b"alpha\nbeta\n");

        Ok(())
    }
}
