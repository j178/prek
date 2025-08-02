use std::collections::HashMap;

use anyhow::Result;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

pub(crate) async fn fix_end_of_file(
    _hook: &Hook,
    filenames: &[&String],
    _env_vars: &HashMap<&'static str, String>,
) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| {
            // TODO: avoid reading the whole file into memory
            let content = fs_err::tokio::read(filename).await?;
            fix_file(filename, &content).await
        })
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

async fn fix_file(filename: &str, content: &[u8]) -> Result<(i32, Vec<u8>)> {
    // If the file is empty, do nothing.
    if content.is_empty() {
        return Ok((0, Vec::new()));
    }

    // Find the last character that is not a newline
    let last_non_newline_pos = content.iter().rposition(|&c| c != b'\n' && c != b'\r');

    if let Some(pos) = last_non_newline_pos {
        // File has content other than newlines
        if pos == content.len() - 1 {
            // Last character is not a newline, add one
            let line_ending = detect_line_ending(&content[..=pos]);
            let new_content = [&content[..=pos], line_ending].concat();
            fs_err::tokio::write(filename, &new_content).await?;
            return Ok((1, format!("Fixing {filename}\n").into_bytes()));
        }
        // Last character is a newline, check for excess newlines
        let after_content = &content[pos + 1..];
        let trimmed_after = trim_excess_newlines(after_content);
        if trimmed_after != after_content {
            let new_content = [&content[..=pos], trimmed_after].concat();
            fs_err::tokio::write(filename, &new_content).await?;
            return Ok((1, format!("Fixing {filename}\n").into_bytes()));
        }
    } else {
        // File consists only of newlines - make it empty
        fs_err::tokio::write(filename, b"").await?;
        return Ok((1, format!("Fixing {filename}\n").into_bytes()));
    }

    Ok((0, Vec::new()))
}

/// Trim excess newlines at the end, keeping only one.
fn trim_excess_newlines(content: &[u8]) -> &[u8] {
    if content.is_empty() {
        return content;
    }

    // Since content only contains newlines, just keep the first one
    if content.starts_with(b"\r\n") {
        b"\r\n"
    } else if content.starts_with(b"\n") {
        b"\n"
    } else if content.starts_with(b"\r") {
        b"\r"
    } else {
        // No newlines found (shouldn't happen given the context)
        &content[..0]
    }
}

/// Detect the line ending type used in the file content.
/// Returns the most common line ending, or Unix (\n) as default.
fn detect_line_ending(content: &[u8]) -> &'static [u8] {
    let mut crlf_count = 0;
    let mut lf_count = 0;
    let mut cr_count = 0;

    let mut i = 0;
    while i < content.len() {
        if i + 1 < content.len() && content[i] == b'\r' && content[i + 1] == b'\n' {
            crlf_count += 1;
            i += 2;
        } else if content[i] == b'\n' {
            lf_count += 1;
            i += 1;
        } else if content[i] == b'\r' {
            cr_count += 1;
            i += 1;
        } else {
            i += 1;
        }
    }

    // Return the most common line ending, with preference for CRLF > LF > CR
    if crlf_count > 0 {
        b"\r\n"
    } else if lf_count > 0 {
        b"\n"
    } else if cr_count > 0 {
        b"\r"
    } else {
        // Default to Unix line endings
        b"\n"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use bstr::ByteSlice;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn create_test_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> PathBuf {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await.unwrap();
        file_path
    }

    async fn run_fix_on_file(file_path: &PathBuf) -> (i32, Vec<u8>) {
        let filename = file_path.to_string_lossy().to_string();
        let content = fs_err::tokio::read(file_path).await.unwrap();
        fix_file(&filename, &content).await.unwrap()
    }

    #[tokio::test]
    async fn test_preserve_windows_line_endings() {
        let dir = tempdir().unwrap();

        let content = b"line1\r\nline2\r\nline3";
        let file_path = create_test_file(&dir, "windows_no_eof.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\r\nline2\r\nline3\r\n");
    }

    #[tokio::test]
    async fn test_preserve_unix_line_endings() {
        let dir = tempdir().unwrap();

        let content = b"line1\nline2\nline3";
        let file_path = create_test_file(&dir, "unix_no_eof.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\nline2\nline3\n");
    }

    #[tokio::test]
    async fn test_preserve_old_mac_line_endings() {
        let dir = tempdir().unwrap();

        let content = b"line1\rline2\rline3";
        let file_path = create_test_file(&dir, "mac_no_eof.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\rline2\rline3\r");
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
    async fn test_mixed_line_endings() {
        let dir = tempdir().unwrap();

        // Test file with mixed line endings (should prefer CRLF as it appears first)
        let content = b"line1\r\nline2\nline3\r\nline4";
        let file_path = create_test_file(&dir, "mixed.txt", content).await;

        let (code, output) = run_fix_on_file(&file_path).await;

        assert_eq!(code, 1, "Should fix the file");
        assert!(output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await.unwrap();
        assert_eq!(new_content, b"line1\r\nline2\nline3\r\nline4\r\n");
    }

    #[tokio::test]
    async fn test_detect_line_ending_function() {
        assert_eq!(detect_line_ending(b"line1\r\nline2\r\n"), b"\r\n");
        assert_eq!(detect_line_ending(b"line1\nline2\n"), b"\n");
        assert_eq!(detect_line_ending(b"line1\rline2\r"), b"\r");
        assert_eq!(detect_line_ending(b"line1\r\nline2\nline3\r\n"), b"\r\n");
        assert_eq!(detect_line_ending(b"no line endings"), b"\n");
        // Test empty content (default to LF)
        assert_eq!(detect_line_ending(b""), b"\n");
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
