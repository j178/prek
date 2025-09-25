use std::path::Path;

use anyhow::Result;
use bstr::ByteSlice;
use clap::{Parser, ValueEnum};
use futures::StreamExt;
use rustc_hash::FxHashMap;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

const CRLF: &[u8] = b"\r\n";
const LF: &[u8] = b"\n";
const CR: &[u8] = b"\r";
const ALL_ENDINGS: [&[u8]; 3] = [CR, CRLF, LF];

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    /// Fix mixed line endings by converting to the most common line ending
    /// or a specified line ending.
    #[clap(long, short, value_enum, default_value_t = FixMode::Auto)]
    fix: FixMode,
}

#[derive(Copy, Clone, Debug, Default, ValueEnum)]
#[allow(clippy::upper_case_acronyms)]
enum FixMode {
    /// Automatically determine the most common line ending and use it
    #[default]
    Auto,
    /// Don't fix, just report if mixed line endings are found
    No,
    /// Convert all line endings to LF
    LF,
    /// Convert all line endings to CRLF
    CRLF,
    /// Convert all line endings to CR
    CR,
}

pub(crate) async fn mixed_line_ending(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    let mut results = futures::stream::iter(filenames)
        .map(|filename| fix_file(hook.project().relative_path(), filename, args.fix))
        .buffered(*CONCURRENCY);

    let mut exit_code = 0;
    let mut output = Vec::new();

    while let Some(result) = results.next().await {
        let (c, o) = result?;
        exit_code |= c;
        output.extend(o);
    }

    Ok((exit_code, output))
}

// Process a single file for mixed line endings
async fn fix_file(file_base: &Path, filename: &Path, fix_mode: FixMode) -> Result<(i32, Vec<u8>)> {
    let file_path = file_base.join(filename);
    let contents = fs_err::tokio::read(&file_path).await?;

    // Skip empty files or binary files
    if contents.is_empty() || contents.find_byte(0).is_some() {
        return Ok((0, Vec::new()));
    }

    let counts = count_line_endings(&contents);
    let has_mixed_endings = counts.len() > 1;

    match fix_mode {
        FixMode::No => {
            if has_mixed_endings {
                Ok((
                    1,
                    format!("{}: mixed line endings\n", filename.display()).into_bytes(),
                ))
            } else {
                Ok((0, Vec::new()))
            }
        }
        FixMode::Auto => {
            if !has_mixed_endings {
                return Ok((0, Vec::new()));
            }

            let target_ending = find_most_common_ending(&counts);
            apply_line_ending(&file_path, &contents, target_ending).await?;
            Ok((1, format!("Fixing {}\n", filename.display()).into_bytes()))
        }
        _ => {
            let target_ending = match fix_mode {
                FixMode::LF => LF,
                FixMode::CRLF => CRLF,
                FixMode::CR => CR,
                _ => unreachable!(),
            };
            let needs_fixing = counts.keys().any(|&ending| ending != target_ending);

            if needs_fixing {
                apply_line_ending(&file_path, &contents, target_ending).await?;
                Ok((1, format!("Fixing {}\n", filename.display()).into_bytes()))
            } else {
                Ok((0, Vec::new()))
            }
        }
    }
}

fn count_line_endings(contents: &[u8]) -> FxHashMap<&'static [u8], usize> {
    let mut counts = FxHashMap::default();

    for line in split_lines_with_endings(contents) {
        let ending = if line.ends_with(CRLF) {
            CRLF
        } else if line.ends_with(CR) {
            CR
        } else if line.ends_with(LF) {
            LF
        } else {
            continue; // Line without ending
        };
        *counts.entry(ending).or_insert(0) += 1;
    }

    counts
}

fn find_most_common_ending(counts: &FxHashMap<&'static [u8], usize>) -> &'static [u8] {
    ALL_ENDINGS
        .iter()
        .max_by_key(|&&ending| counts.get(ending).unwrap_or(&0))
        .copied()
        .unwrap_or(LF)
}

async fn apply_line_ending(filename: &Path, contents: &[u8], ending: &[u8]) -> Result<()> {
    let lines = split_lines_with_endings(contents);
    let mut new_contents = Vec::with_capacity(contents.len());

    for line in lines {
        let line_without_ending = strip_line_ending(line);
        new_contents.extend_from_slice(line_without_ending);
        new_contents.extend_from_slice(ending);
    }

    fs_err::tokio::write(filename, &new_contents).await?;
    Ok(())
}

fn strip_line_ending(line: &[u8]) -> &[u8] {
    if line.ends_with(CRLF) {
        &line[..line.len() - 2]
    } else if line.ends_with(LF) || line.ends_with(CR) {
        &line[..line.len() - 1]
    } else {
        line
    }
}

fn split_lines_with_endings(contents: &[u8]) -> Vec<&[u8]> {
    if contents.is_empty() {
        return Vec::new();
    }

    let mut lines = Vec::new();
    let mut last_end = 0;
    let mut i = 0;

    while i < contents.len() {
        match contents[i] {
            b'\n' => {
                lines.push(&contents[last_end..=i]);
                last_end = i + 1;
                i += 1;
            }
            b'\r' => {
                if i + 1 < contents.len() && contents[i + 1] == b'\n' {
                    // CRLF
                    lines.push(&contents[last_end..=i + 1]);
                    last_end = i + 2;
                    i += 2;
                } else {
                    // CR
                    lines.push(&contents[last_end..=i]);
                    last_end = i + 1;
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    // Add remaining content if any
    if last_end < contents.len() {
        lines.push(&contents[last_end..]);
    }

    lines
}

#[cfg(test)]
mod tests {
    use super::*;
    use bstr::ByteSlice;
    use std::path::{Path, PathBuf};
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

    #[tokio::test]
    async fn test_auto_fix_crlf_wins() -> Result<()> {
        let dir = tempdir()?;
        let content = b"line1\nline2\r\nline3\r\n"; // 1 LF, 2 CRLF
        let file_path = create_test_file(&dir, "mixed_crlf.txt", content).await?;
        let (code, output) = fix_file(Path::new(""), &file_path, FixMode::Auto).await?;
        assert_eq!(code, 1);
        assert!(output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\r\nline2\r\nline3\r\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_auto_fix_lf_wins() -> Result<()> {
        let dir = tempdir()?;
        let content = b"line1\nline2\nline3\r\n"; // 2 LF, 1 CRLF
        let file_path = create_test_file(&dir, "mixed_lf.txt", content).await?;
        let (code, output) = fix_file(Path::new(""), &file_path, FixMode::Auto).await?;
        assert_eq!(code, 1);
        assert!(output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\nline3\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_auto_fix_tie_prefers_lf() -> Result<()> {
        let dir = tempdir()?;
        let content = b"line1\nline2\r\n"; // 1 LF, 1 CRLF
        let file_path = create_test_file(&dir, "mixed_tie.txt", content).await?;
        let (code, output) = fix_file(Path::new(""), &file_path, FixMode::Auto).await?;
        assert_eq!(code, 1);
        assert!(output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_fix_no() -> Result<()> {
        let dir = tempdir()?;
        let content = b"line1\nline2\r\n";
        let file_path = create_test_file(&dir, "mixed_no.txt", content).await?;
        let (code, output) = fix_file(Path::new(""), &file_path, FixMode::No).await?;
        assert_eq!(code, 1);
        assert!(output.as_bytes().contains_str("mixed line endings"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content); // File should not be changed

        Ok(())
    }

    #[tokio::test]
    async fn test_no_line_endings() -> Result<()> {
        let dir = tempdir()?;
        let content = b"some content";
        let file_path = create_test_file(&dir, "no_endings.txt", content).await?;
        let (code, output) = fix_file(Path::new(""), &file_path, FixMode::Auto).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_fix_with_cr_endings() -> Result<()> {
        let dir = tempdir()?;
        // A file with a mix of all three line ending types
        let content = b"line1\rline2\nline3\r\n";
        let file_path = create_test_file(&dir, "all_mixed.txt", content).await?;

        // Test auto fix (should prefer LF as it's a 3-way tie)
        let (code, output) = fix_file(Path::new(""), &file_path, FixMode::Auto).await?;
        assert_eq!(code, 1);
        assert!(output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\nline3\n");

        // Restore content and test fix to CRLF
        fs_err::tokio::write(&file_path, content).await?;
        let (code, output) = fix_file(Path::new(""), &file_path, FixMode::CRLF).await?;
        assert_eq!(code, 1);
        assert!(output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\r\nline2\r\nline3\r\n");

        Ok(())
    }
}
