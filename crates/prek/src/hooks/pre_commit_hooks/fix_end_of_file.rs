use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::Result;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{FixArgs, parse_hook_args, run_file_checks};
use crate::run::INTERNAL_CONCURRENCY;

/// Runs the `end-of-file-fixer` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: FixArgs = parse_hook_args(hook)?;
    run_file_checks(
        &args.filenames,
        filenames,
        *INTERNAL_CONCURRENCY,
        |filename| fix_file(hook.project().relative_path(), filename, args.check),
    )
    .await
}

async fn fix_file(file_base: &Path, filename: &Path, check: bool) -> Result<HookOutput> {
    let file_path = file_base.join(filename);
    // Keep a file's blocking I/O in one task instead of re-entering the blocking pool per operation.
    let needs_fix = tokio::task::spawn_blocking(move || fix_file_sync(&file_path, check)).await??;
    if needs_fix {
        let action = if check { "Would fix" } else { "Fixing" };
        Ok(HookOutput::known(
            1,
            format!("{action} {}\n", filename.display()).into_bytes(),
            !check,
        ))
    } else {
        Ok(HookOutput::unchanged(0, Vec::new()))
    }
}

fn fix_file_sync(file_path: &Path, check: bool) -> Result<bool> {
    // If the file is empty, do nothing and avoid opening a write handle.
    let file_size = fs_err::metadata(file_path)?.len();
    if file_size == 0 {
        return Ok(false);
    }

    let mut file = fs_err::OpenOptions::new()
        .read(true)
        .write(!check)
        .open(file_path)?;

    let fixed_len = fixed_file_len(&mut file, file_size)?;
    if fixed_len == file_size {
        return Ok(false);
    }

    if !check {
        if fixed_len > file_size {
            file.seek(SeekFrom::Start(file_size))?;
            file.write_all(b"\n")?;
        } else {
            file.set_len(fixed_len)?;
        }
    }
    Ok(true)
}

fn fixed_file_len(reader: &mut (impl Read + Seek), file_size: u64) -> Result<u64> {
    let mut buf = [0u8; 4096];
    let mut offset = file_size;
    let mut ending_len = 0;
    let mut next_byte = None;

    while offset > 0 {
        let block_size = usize::try_from(offset.min(buf.len() as u64))?;
        offset -= block_size as u64;
        reader.seek(SeekFrom::Start(offset))?;
        reader.read_exact(&mut buf[..block_size])?;

        for (index, &byte) in buf[..block_size].iter().enumerate().rev() {
            if !matches!(byte, b'\r' | b'\n') {
                // Files without a trailing line ending need an appended LF.
                return Ok(offset + index as u64 + 1 + ending_len.max(1));
            }
            ending_len = if byte == b'\r' && next_byte == Some(b'\n') {
                2
            } else {
                1
            };
            // Preserve CRLF detection across block boundaries.
            next_byte = Some(byte);
        }
    }

    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::Ok;
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
    async fn test_long_trailing_newlines_preserve_contents() -> Result<()> {
        let dir = tempdir()?;
        for ending in [b"\n".as_slice(), b"\r\n", b"\r"] {
            for count in [4095, 4096, 4097, 8193] {
                let content = [b"keep", ending.repeat(count).as_slice()].concat();
                let path = create_test_file(&dir, "long-tail.txt", &content).await?;

                let result = fix_file(Path::new(""), &path, true).await?;
                assert_eq!(result.exit_status, 1);
                assert_eq!(fs_err::tokio::read(&path).await?, content);

                let result = fix_file(Path::new(""), &path, false).await?;
                assert_eq!(result.exit_status, 1);
                assert_eq!(
                    fs_err::tokio::read(&path).await?,
                    [b"keep", ending].concat()
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_no_line_ending_1() -> Result<()> {
        let dir = tempdir()?;

        // For files without line endings, just append "\n" at the end, no matter
        // what line endings are previously used.
        // This is consistent with the behavior of `pre-commit`.

        let content = b"line1\nline2\nline3";
        let file_path = create_test_file(&dir, "unix_no_eof.txt", content).await?;
        let result = fix_file(Path::new(""), &file_path, false).await?;
        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\nline3\n");

        let content = b"line1\r\nline2\nline3\r\nline4";
        let file_path = create_test_file(&dir, "mixed.txt", content).await?;
        let result = fix_file(Path::new(""), &file_path, false).await?;
        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\r\nline2\nline3\r\nline4\n");

        let content = b"line1\r\nline2\r\nline3";
        let file_path = create_test_file(&dir, "windows_no_eof.txt", content).await?;
        let result = fix_file(Path::new(""), &file_path, false).await?;
        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\r\nline2\r\nline3\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_already_has_correct_windows_ending() -> Result<()> {
        let dir = tempdir()?;

        let content = b"line1\r\nline2\r\nline3\r\n";
        let file_path = create_test_file(&dir, "windows_with_eof.txt", content).await?;

        let result = fix_file(Path::new(""), &file_path, false).await?;

        assert_eq!(result.exit_status, 0, "Should not change the file");
        assert!(result.output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_already_has_correct_unix_ending() -> Result<()> {
        let dir = tempdir()?;

        let content = b"line1\nline2\nline3\n";
        let file_path = create_test_file(&dir, "unix_with_eof.txt", content).await?;

        let result = fix_file(Path::new(""), &file_path, false).await?;

        assert_eq!(result.exit_status, 0, "Should not change the file");
        assert!(result.output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<()> {
        let dir = tempdir()?;

        let content = b"";
        let file_path = create_test_file(&dir, "empty.txt", content).await?;

        let result = fix_file(Path::new(""), &file_path, false).await?;

        assert_eq!(result.exit_status, 0, "Should not change empty file");
        assert!(result.output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_excess_newlines_removal() -> Result<()> {
        let dir = tempdir()?;

        let content = b"line1\nline2\n\n\n\n";
        let file_path = create_test_file(&dir, "excess_newlines.txt", content).await?;

        let result = fix_file(Path::new(""), &file_path, false).await?;

        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_excess_crlf_removal() -> Result<()> {
        let dir = tempdir()?;

        let content = b"line1\r\nline2\r\n\r\n\r\n";
        let file_path = create_test_file(&dir, "excess_crlf.txt", content).await?;

        let result = fix_file(Path::new(""), &file_path, false).await?;

        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\r\nline2\r\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_all_newlines_make_empty() -> Result<()> {
        let dir = tempdir()?;

        let content = b"\n\n\n\n";
        let file_path = create_test_file(&dir, "only_newlines.txt", content).await?;

        let result = fix_file(Path::new(""), &file_path, false).await?;

        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"");

        Ok(())
    }
}
