use std::path::Path;

use anyhow::Result;
use bstr::ByteSlice;
use clap::{Parser, ValueEnum};

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;

const CRLF: &[u8] = b"\r\n";
const LF: &[u8] = b"\n";
const CR: &[u8] = b"\r";

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

#[derive(Default)]
struct LineEndingCounts {
    cr: usize,
    crlf: usize,
    lf: usize,
}

impl LineEndingCounts {
    fn kind_count(&self) -> usize {
        usize::from(self.cr > 0) + usize::from(self.crlf > 0) + usize::from(self.lf > 0)
    }

    fn has_any_except(&self, ending: &[u8]) -> bool {
        (ending != CR && self.cr > 0)
            || (ending != CRLF && self.crlf > 0)
            || (ending != LF && self.lf > 0)
    }
}

pub(crate) async fn mixed_line_ending(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.expect_direct().split()?.iter().chain(&hook.args))?;

    run_concurrent_file_checks(
        filenames.iter().copied(),
        *INTERNAL_CONCURRENCY,
        |filename| fix_file(hook.project().relative_path(), filename, args.fix),
    )
    .await
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
    let has_mixed_endings = counts.kind_count() > 1;

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
            let needs_fixing = counts.has_any_except(target_ending);

            if needs_fixing {
                apply_line_ending(&file_path, &contents, target_ending).await?;
                Ok((1, format!("Fixing {}\n", filename.display()).into_bytes()))
            } else {
                Ok((0, Vec::new()))
            }
        }
    }
}

fn count_line_endings(contents: &[u8]) -> LineEndingCounts {
    let mut counts = LineEndingCounts::default();
    let mut index = 0;

    while index < contents.len() {
        match contents[index] {
            b'\r' if contents.get(index + 1) == Some(&b'\n') => {
                counts.crlf += 1;
                index += 2;
            }
            b'\r' => {
                counts.cr += 1;
                index += 1;
            }
            b'\n' => {
                counts.lf += 1;
                index += 1;
            }
            _ => index += 1,
        }
    }

    counts
}

fn find_most_common_ending(counts: &LineEndingCounts) -> &'static [u8] {
    // Preserve the previous tie-break order from `[CR, CRLF, LF].max_by_key(...)`.
    if counts.lf >= counts.crlf && counts.lf >= counts.cr {
        LF
    } else if counts.crlf >= counts.cr {
        CRLF
    } else {
        CR
    }
}

async fn apply_line_ending(filename: &Path, contents: &[u8], ending: &[u8]) -> Result<()> {
    let mut new_contents = Vec::with_capacity(contents.len());
    let mut line_start = 0;
    let mut index = 0;

    while index < contents.len() {
        match contents[index] {
            b'\r' if contents.get(index + 1) == Some(&b'\n') => {
                new_contents.extend_from_slice(&contents[line_start..index]);
                new_contents.extend_from_slice(ending);
                index += 2;
                line_start = index;
            }
            b'\r' | b'\n' => {
                new_contents.extend_from_slice(&contents[line_start..index]);
                new_contents.extend_from_slice(ending);
                index += 1;
                line_start = index;
            }
            _ => index += 1,
        }
    }

    if line_start < contents.len() {
        new_contents.extend_from_slice(&contents[line_start..]);
        new_contents.extend_from_slice(ending);
    }

    fs_err::tokio::write(filename, &new_contents).await?;
    Ok(())
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
