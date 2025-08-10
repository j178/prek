use anyhow::Result;
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWriteExt, SeekFrom};

use crate::hook::Hook;
use crate::run::CONCURRENCY;

pub(crate) async fn fix_end_of_file(_hook: &Hook, filenames: &[&String]) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| fix_file(filename).await)
        .buffered(*CONCURRENCY);

    let mut code = 0;
    let mut output = Vec::new();

    while let Some(result) = tasks.next().await {
        let (c, o) = result?;
        code |= c;
        output.extend(o);
    }

    Ok((code, output))
}

async fn fix_file(filename: &str) -> Result<(i32, Vec<u8>)> {
    let mut file = fs_err::tokio::OpenOptions::new()
        .read(true)
        .write(true)
        .open(filename)
        .await?;

    // If the file is empty, do nothing.
    let file_size = file.metadata().await?.len();
    if file_size == 0 {
        return Ok((0, Vec::new()));
    }

    match find_last_non_ending(&mut file).await? {
        (None, _) => {
            // File contains only line endings, so we can just set it to empty.
            file.set_len(0).await?;
            file.flush().await?;
            file.shutdown().await?;
            Ok((1, format!("Fixing {filename}\n").into_bytes()))
        }
        (Some(pos), None) => {
            // File has some content, but no line ending at the end.
            file.seek(SeekFrom::Start(pos + 1)).await?;
            file.write_all(b"\n").await?;
            file.flush().await?;
            file.shutdown().await?;
            Ok((1, format!("Fixing {filename}\n").into_bytes()))
        }
        (Some(pos), Some(line_ending)) => {
            // File has some content and at least one line ending.
            let new_size = pos + 1 + line_ending.len() as u64;
            if file_size == new_size {
                // File already has the correct line ending.
                return Ok((0, Vec::new()));
            }
            file.set_len(new_size).await?;
            Ok((1, format!("Fixing {filename}\n").into_bytes()))
        }
    }
}

fn determine_line_ending(first: u8, second: u8) -> Option<&'static str> {
    if first == b'\r' && second == b'\n' {
        Some("\r\n")
    } else if first == b'\n' {
        Some("\n")
    } else if first == b'\r' {
        Some("\r")
    } else {
        None
    }
}

/// Searches for the last non-line-ending character in the file.
/// Returns the position of the last non-line-ending character and the line ending type.
async fn find_last_non_ending<T>(reader: &mut T) -> Result<(Option<u64>, Option<&str>)>
where
    T: AsyncRead + AsyncSeek + Unpin,
{
    const MAX_SCAN_SIZE: usize = 4 * 1024; // 4KB

    let data_len = reader.seek(SeekFrom::End(0)).await?;
    if data_len == 0 {
        return Ok((None, None));
    }

    let mut read_len = 0;
    let mut next_char = 0;
    let mut buf = vec![0u8; MAX_SCAN_SIZE];
    let mut line_ending = None;

    while read_len < data_len {
        let block_size = MAX_SCAN_SIZE.min(usize::try_from(data_len - read_len)?);
        // SAFETY: block_size is guaranteed to be less than or equal to MAX_SCAN_SIZE
        reader
            .seek(SeekFrom::Current(-i64::try_from(block_size).unwrap()))
            .await?;
        reader.read_exact(&mut buf[..block_size]).await?;
        read_len += block_size as u64;

        let mut pos = block_size;
        while pos > 0 {
            pos -= 1;

            if matches!(buf[pos], b'\n' | b'\r') {
                line_ending = if pos + 1 == block_size {
                    determine_line_ending(buf[pos], next_char)
                } else {
                    determine_line_ending(buf[pos], buf[pos + 1])
                };
            } else {
                return Ok((Some(data_len - read_len + pos as u64), line_ending));
            }
        }

        next_char = buf[0];
    }

    Ok((None, line_ending))
}

#[cfg(test)]
mod tests {
    use super::*;

    use bstr::ByteSlice;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    async fn create_test_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> PathBuf {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await.unwrap();
        file_path
    }

    async fn run_fix_on_file(file_path: &Path) -> (i32, Vec<u8>) {
        let filename = file_path.to_string_lossy().to_string();
        fix_file(&filename).await.unwrap()
    }

    #[tokio::test]
    async fn test_no_line_ending_1() {
        let dir = tempdir().unwrap();

        // For files without line endings, just append "\n" at the end, no matter
        // what line endings are previously used.
        // This is consistent with the behavior of `pre-commit`.

        let content = b"line1\nline2\nline3";
        let file_path = create_test_file(&dir, "unix_no_eof.txt", content).await;
        let (code, output) = run_fix_on_file(&file_path).await;
        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\nline2\nline3\n");

        let content = b"line1\r\nline2\nline3\r\nline4";
        let file_path = create_test_file(&dir, "mixed.txt", content).await;
        let (code, output) = run_fix_on_file(&file_path).await;
        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\r\nline2\nline3\r\nline4\n");

        let content = b"line1\r\nline2\r\nline3";
        let file_path = create_test_file(&dir, "windows_no_eof.txt", content).await;
        let (code, output) = run_fix_on_file(&file_path).await;
        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\r\nline2\r\nline3\n");
    }

    #[tokio::test]
    async fn test_already_has_correct_windows_ending() {
        let dir = tempdir().unwrap();

        let content = b"line1\r\nline2\r\nline3\r\n";
        let file_path = create_test_file(&dir, "windows_with_eof.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 0, "Should not change the file");
        assert!(output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, content);
    }

    #[tokio::test]
    async fn test_already_has_correct_unix_ending() {
        let dir = tempdir().unwrap();

        let content = b"line1\nline2\nline3\n";
        let file_path = create_test_file(&dir, "unix_with_eof.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 0, "Should not change the file");
        assert!(output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, content);
    }

    #[tokio::test]
    async fn test_empty_file() {
        let dir = tempdir().unwrap();

        let content = b"";
        let file_path = create_test_file(&dir, "empty.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 0, "Should not change empty file");
        assert!(output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"");
    }

    #[tokio::test]
    async fn test_excess_newlines_removal() {
        let dir = tempdir().unwrap();

        let content = b"line1\nline2\n\n\n\n";
        let file_path = create_test_file(&dir, "excess_newlines.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\nline2\n");
    }

    #[tokio::test]
    async fn test_excess_crlf_removal() {
        let dir = tempdir().unwrap();

        let content = b"line1\r\nline2\r\n\r\n\r\n";
        let file_path = create_test_file(&dir, "excess_crlf.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\r\nline2\r\n");
    }

    #[tokio::test]
    async fn test_all_newlines_make_empty() {
        let dir = tempdir().unwrap();

        let content = b"\n\n\n\n";
        let file_path = create_test_file(&dir, "only_newlines.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"");
    }
}
