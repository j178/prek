use std::path::Path;

use anyhow::Result;
use clap::Parser;
use tokio::io::AsyncBufReadExt;

use crate::git::get_git_dir;
use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::run::CONCURRENCY;

const START_PATTERN: &[u8] = b"<<<<<<< ";
const ANCESTOR_PATTERN: &[u8] = b"||||||| ";
const END_PATTERN: &[u8] = b">>>>>>> ";
const SEPARATOR_PATTERNS: &[&[u8]] = &[b"======= ", b"=======\r\n", b"=======\n"];

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long)]
    assume_in_merge: bool,
}

pub(crate) async fn check_merge_conflict(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.expect_direct().split()?.iter().chain(&hook.args))?;

    // Check if we're in a merge state or assuming merge
    if !args.assume_in_merge && !is_in_merge().await? {
        return Ok((0, Vec::new()));
    }

    run_concurrent_file_checks(filenames.iter().copied(), *CONCURRENCY, |filename| {
        check_file(hook.project().relative_path(), filename)
    })
    .await
}

async fn is_in_merge() -> Result<bool> {
    // Change directory temporarily or ensure we're in the right directory
    let git_dir = get_git_dir().await?;

    // Check if MERGE_MSG exists
    let merge_msg_exists = git_dir.join("MERGE_MSG").exists();
    if !merge_msg_exists {
        return Ok(false);
    }

    // Check if any of the merge state files exist
    Ok(git_dir.join("MERGE_HEAD").exists()
        || git_dir.join("rebase-apply").exists()
        || git_dir.join("rebase-merge").exists())
}

async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let file_path = file_base.join(filename);
    let file = fs_err::tokio::File::open(&file_path).await?;
    let mut reader = tokio::io::BufReader::new(file);

    let mut code = 0;
    let mut output = Vec::new();
    let mut line = Vec::new();
    let mut line_number = 1;
    let mut in_conflict = false;

    let mut report_conflict = |line_number: usize, pattern: &str| {
        output.extend(conflict_message(filename, line_number, pattern));
        code = 1;
    };

    while reader.read_until(b'\n', &mut line).await? != 0 {
        if line.starts_with(START_PATTERN) {
            report_conflict(line_number, "<<<<<<< ");
            in_conflict = true;
        } else if in_conflict && line.starts_with(ANCESTOR_PATTERN) {
            report_conflict(line_number, "||||||| ");
        } else if in_conflict
            && SEPARATOR_PATTERNS
                .iter()
                .any(|pattern| line.starts_with(pattern))
        {
            report_conflict(line_number, "=======");
        } else if line.starts_with(END_PATTERN) {
            report_conflict(line_number, ">>>>>>> ");
            in_conflict = false;
        }

        line.clear();
        line_number += 1;
    }

    Ok((code, output))
}

fn conflict_message(filename: &Path, line_number: usize, pattern: &str) -> Vec<u8> {
    format!(
        "{}:{line_number}: Merge conflict string {pattern:?} found\n",
        filename.display(),
    )
    .into_bytes()
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

    #[tokio::test]
    async fn test_no_conflict_markers() -> Result<()> {
        let dir = tempdir()?;
        let content = b"This is a normal file\nWith no conflict markers\n";
        let file_path = create_test_file(&dir, "clean.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_start() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\n<<<<<<< HEAD\nConflicting line\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("<<<<<<< "));
        assert!(output_str.contains("conflict.txt:2"));
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_end() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\n>>>>>>> branch\nMore content\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains(">>>>>>> "));
        Ok(())
    }

    #[tokio::test]
    async fn test_full_conflict_block() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Before conflict\n<<<<<<< HEAD\nOur changes\n=======\nTheir changes\n>>>>>>> branch\nAfter conflict\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        // Should find all three markers
        assert!(output_str.contains("<<<<<<< "));
        assert!(output_str.contains("======="));
        assert!(output_str.contains(">>>>>>> "));
        Ok(())
    }

    #[tokio::test]
    async fn test_diff3_conflict_block() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Before conflict\n<<<<<<< HEAD\nOur changes\n||||||| base\nCommon ancestor\n=======\nTheir changes\n>>>>>>> branch\nAfter conflict\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("<<<<<<< "));
        assert!(output_str.contains("||||||| "));
        assert!(output_str.contains("======="));
        assert!(output_str.contains(">>>>>>> "));
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_not_at_start() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content <<<<<<< HEAD\n";
        let file_path = create_test_file(&dir, "no_conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        // Should not detect conflict since marker is not at line start
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_crlf() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\r\n<<<<<<< HEAD\r\nConflicting line\r\n=======\r\nOther line\r\n>>>>>>> branch\r\n";
        let file_path = create_test_file(&dir, "conflict_crlf.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_lf() -> Result<()> {
        let dir = tempdir()?;
        let content =
            b"Some content\n<<<<<<< HEAD\nConflicting line\n=======\nOther line\n>>>>>>> branch\n";
        let file_path = create_test_file(&dir, "conflict_lf.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_separator_reported_without_conflict_end() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Before conflict\n<<<<<<< HEAD\nOur changes\n=======\n";
        let file_path = create_test_file(&dir, "partial_conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("<<<<<<< "));
        assert!(output_str.contains("======="));
        Ok(())
    }

    #[tokio::test]
    async fn test_ancestor_not_reported_without_conflict_start() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Before conflict\n||||||| base\n";
        let file_path = create_test_file(&dir, "partial_conflict.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_rst_heading_is_not_treated_as_conflict() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Depends\n=======\n";
        let file_path = create_test_file(&dir, "doc.rst", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_conflicts() -> Result<()> {
        let dir = tempdir()?;
        let content = b"<<<<<<< HEAD\nFirst\n=======\nSecond\n>>>>>>> branch\nMiddle\n<<<<<<< HEAD\nThird\n=======\nFourth\n>>>>>>> other\n";
        let file_path = create_test_file(&dir, "multiple.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        // Should find all markers from both conflicts (one per line with marker)
        let marker_count = output_str.matches("Merge conflict string").count();
        assert_eq!(marker_count, 6); // 3 markers per conflict * 2 conflicts
        Ok(())
    }

    #[tokio::test]
    async fn test_binary_file_with_conflict() -> Result<()> {
        let dir = tempdir()?;
        let mut content = vec![0xFF, 0xFE, 0xFD];
        content.extend_from_slice(b"\n<<<<<<< HEAD\n");
        let file_path = create_test_file(&dir, "binary.bin", &content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }
}
