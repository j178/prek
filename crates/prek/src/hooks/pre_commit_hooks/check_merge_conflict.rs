use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use memchr::{memchr_iter, memmem};

use crate::git::git_dir;
use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{parse_hook_args, run_blocking_file_checks};

const START_PATTERN: &[u8] = b"<<<<<<< ";
const ANCESTOR_PATTERN: &[u8] = b"||||||| ";
const END_PATTERN: &[u8] = b">>>>>>> ";
const SEPARATOR_PATTERNS: &[&[u8]] = &[b"======= ", b"=======\r\n", b"=======\n"];

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Run even when no merge or rebase is detected.
    #[arg(long)]
    assume_in_merge: bool,
    #[arg(value_name = "FILENAMES")]
    filenames: Vec<PathBuf>,
}

/// Runs the `check-merge-conflict` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;

    // Check if we're in a merge state or assuming merge
    if !args.assume_in_merge && !is_in_merge().await? {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        check_file,
    )
    .await
}

async fn is_in_merge() -> Result<bool> {
    let git_dir = git_dir().await?;

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

fn check_file(file_path: &Path, display_path: &Path) -> Result<HookOutput> {
    // Keep enough lookahead for the longest marker, "=======\r\n".
    const LOOKAHEAD: usize = 8;
    let mut file = fs_err::File::open(file_path)?;
    let start_marker = memmem::Finder::new(b"\n<<<<<<< ");
    let end_marker = memmem::Finder::new(b"\n>>>>>>> ");
    let mut buf = vec![0u8; 32768 + LOOKAHEAD];
    let mut carry_len = 0;
    let mut code = 0;
    let mut output = Vec::new();
    let mut line_number = 1;
    let mut at_line_start = true;
    let mut in_conflict = false;

    let mut report_conflict = |line_number: usize, pattern: &str| -> std::io::Result<()> {
        write_conflict_message(&mut output, display_path, line_number, pattern)?;
        code = 1;
        Ok(())
    };

    loop {
        let read = match file.read(&mut buf[carry_len..]) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        let len = carry_len + read;
        let consumed = if read == 0 {
            len
        } else {
            len.saturating_sub(LOOKAHEAD)
        };
        let content = &buf[..len];
        let newlines = memchr_iter(b'\n', &content[..consumed]);

        if in_conflict
            || (at_line_start
                && (content.starts_with(START_PATTERN) || content.starts_with(END_PATTERN)))
            || start_marker.find(content).is_some()
            || end_marker.find(content).is_some()
        {
            let mut check_line = |index: usize, line_number| -> std::io::Result<()> {
                let line = &content[index..];
                if line.starts_with(START_PATTERN) {
                    report_conflict(line_number, "<<<<<<< ")?;
                    in_conflict = true;
                } else if in_conflict && line.starts_with(ANCESTOR_PATTERN) {
                    report_conflict(line_number, "||||||| ")?;
                } else if in_conflict
                    && SEPARATOR_PATTERNS
                        .iter()
                        .any(|pattern| line.starts_with(pattern))
                {
                    report_conflict(line_number, "=======")?;
                } else if line.starts_with(END_PATTERN) {
                    report_conflict(line_number, ">>>>>>> ")?;
                    in_conflict = false;
                }
                Ok(())
            };
            if consumed > 0 && at_line_start {
                check_line(0, line_number)?;
            }
            for index in newlines {
                line_number += 1;
                if index + 1 < consumed {
                    check_line(index + 1, line_number)?;
                }
            }
        } else {
            // Most files have no conflict markers, so count whole blocks instead of visiting lines.
            line_number += newlines.count();
        }
        if consumed > 0 {
            at_line_start = content[consumed - 1] == b'\n';
        }
        if read == 0 {
            break;
        }
        carry_len = len - consumed;
        buf.copy_within(consumed..len, 0);
    }

    Ok(HookOutput::unchanged(code, output))
}

fn write_conflict_message(
    output: &mut Vec<u8>,
    display_path: &Path,
    line_number: usize,
    pattern: &str,
) -> std::io::Result<()> {
    writeln!(
        output,
        "{}:{line_number}: Merge conflict string {pattern:?} found",
        display_path.display(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn conflict_markers_across_read_boundaries() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("conflict.txt");
        for padding in [8190, 8191, 8192, 16355, 16364, 16383, 32763] {
            let mut content = vec![b'x'; padding];
            content.extend_from_slice(b"\n<<<<<<< HEAD\r\n");
            content.extend_from_slice(&vec![b'x'; 16384]);
            content.extend_from_slice(b"\n=======\r\n>>>>>>> branch");
            fs_err::write(&path, content)?;

            let result = check_file(&path, Path::new("conflict.txt"))?;
            assert_eq!(result.exit_status, 1);
            assert_eq!(
                String::from_utf8(result.output)?,
                concat!(
                    "conflict.txt:2: Merge conflict string \"<<<<<<< \" found\n",
                    "conflict.txt:4: Merge conflict string \"=======\" found\n",
                    "conflict.txt:5: Merge conflict string \">>>>>>> \" found\n",
                ),
            );
        }
        Ok(())
    }

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
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_start() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\n<<<<<<< HEAD\nConflicting line\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        let output_str = String::from_utf8_lossy(&result.output);
        assert!(output_str.contains("<<<<<<< "));
        assert!(output_str.contains("conflict.txt:2"));
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_end() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\n>>>>>>> branch\nMore content\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        let output_str = String::from_utf8_lossy(&result.output);
        assert!(output_str.contains(">>>>>>> "));
        Ok(())
    }

    #[tokio::test]
    async fn test_full_conflict_block() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Before conflict\n<<<<<<< HEAD\nOur changes\n=======\nTheir changes\n>>>>>>> branch\nAfter conflict\n";
        let file_path = create_test_file(&dir, "conflict.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        let output_str = String::from_utf8_lossy(&result.output);
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
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        let output_str = String::from_utf8_lossy(&result.output);
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
        let result = check_file(&file_path, &file_path)?;
        // Should not detect conflict since marker is not at line start
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_crlf() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Some content\r\n<<<<<<< HEAD\r\nConflicting line\r\n=======\r\nOther line\r\n>>>>>>> branch\r\n";
        let file_path = create_test_file(&dir, "conflict_crlf.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[tokio::test]
    async fn test_conflict_marker_lf() -> Result<()> {
        let dir = tempdir()?;
        let content =
            b"Some content\n<<<<<<< HEAD\nConflicting line\n=======\nOther line\n>>>>>>> branch\n";
        let file_path = create_test_file(&dir, "conflict_lf.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[tokio::test]
    async fn test_separator_reported_without_conflict_end() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Before conflict\n<<<<<<< HEAD\nOur changes\n=======\n";
        let file_path = create_test_file(&dir, "partial_conflict.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        let output_str = String::from_utf8_lossy(&result.output);
        assert!(output_str.contains("<<<<<<< "));
        assert!(output_str.contains("======="));
        Ok(())
    }

    #[tokio::test]
    async fn test_ancestor_not_reported_without_conflict_start() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Before conflict\n||||||| base\n";
        let file_path = create_test_file(&dir, "partial_conflict.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[tokio::test]
    async fn test_rst_heading_is_not_treated_as_conflict() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Depends\n=======\n";
        let file_path = create_test_file(&dir, "doc.rst", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_conflicts() -> Result<()> {
        let dir = tempdir()?;
        let content = b"<<<<<<< HEAD\nFirst\n=======\nSecond\n>>>>>>> branch\nMiddle\n<<<<<<< HEAD\nThird\n=======\nFourth\n>>>>>>> other\n";
        let file_path = create_test_file(&dir, "multiple.txt", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        let output_str = String::from_utf8_lossy(&result.output);
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
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }
}
