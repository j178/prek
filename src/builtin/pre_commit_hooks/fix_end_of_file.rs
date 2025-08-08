use crate::hook::Hook;
use crate::run::CONCURRENCY;
use anyhow::Result;
use futures::StreamExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeek, AsyncSeekExt, AsyncWriteExt, SeekFrom};

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

#[derive(Default)]
struct LineEndingDetector {
    final_pos: u64,
    crlf_count: usize,
    lf_count: usize,
    cr_count: usize,
}

impl LineEndingDetector {
    pub async fn from_reader<T, F>(reader: &mut T, scan_stop_strategy: F) -> Result<Self>
    where
        T: AsyncRead + AsyncSeek + Unpin,
        F: ScanStopStrategy,
    {
        const MAX_SCAN_SIZE: usize = 8 * 1024;

        Self::from_reader_with_block_size(reader, scan_stop_strategy, MAX_SCAN_SIZE).await
    }

    async fn from_reader_with_block_size<T, F>(
        reader: &mut T,
        scan_stop_strategy: F,
        max_scan_size: usize,
    ) -> Result<Self>
    where
        T: AsyncRead + AsyncSeek + Unpin,
        F: ScanStopStrategy,
    {
        const MAX_ALLOWED_SCAN_SIZE: usize = 4 * 1024 * 1024; // 4MB

        if max_scan_size == 0 || max_scan_size > MAX_ALLOWED_SCAN_SIZE {
            return Err(anyhow::anyhow!(format!(
                "max_scan_size must be between 1 and {} bytes",
                MAX_ALLOWED_SCAN_SIZE
            )));
        }

        let mut line_ending_detector = Self::default();
        let data_len = reader.seek(SeekFrom::End(0)).await?;
        if data_len == 0 {
            return Ok(line_ending_detector);
        }

        let mut pre_tail_buf = [0u8; 1];
        let mut read_len = 0;
        let mut buf = vec![0u8; max_scan_size];

        while read_len < data_len {
            let block_size = max_scan_size.min(usize::try_from(data_len - read_len)?);
            line_ending_detector
                .read_bytes_backward(reader, &mut buf[..block_size], false)
                .await?;
            read_len += block_size as u64;

            // Cache last byte of previous block to avoid splitting `b"\r\n"`.
            if read_len != data_len {
                line_ending_detector
                    .read_bytes_backward(reader, &mut pre_tail_buf, true)
                    .await?;
            }

            let mut pos = block_size;
            while pos > 0 {
                pos -= 1;
                if scan_stop_strategy.should_stop(buf[pos], pos) {
                    line_ending_detector.final_pos = data_len - read_len + pos as u64;
                    return Ok(line_ending_detector);
                }

                if buf[pos] == b'\n' {
                    if pos > 0 && buf[pos - 1] == b'\r' {
                        line_ending_detector.crlf_count += 1;

                        pos -= 1;
                    } else if pos == 0 && pre_tail_buf[0] == b'\r' {
                        line_ending_detector.crlf_count += 1;
                        reader.seek(SeekFrom::Current(-1)).await?;
                        read_len += 1;
                    } else {
                        line_ending_detector.lf_count += 1;
                    }
                } else if buf[pos] == b'\r' {
                    line_ending_detector.cr_count += 1;
                }
            }
        }
        Ok(line_ending_detector)
    }
}

impl LineEndingDetector {
    pub fn dominant_line_ending(&self) -> &'static [u8] {
        if self.crlf_count > self.cr_count && self.crlf_count > self.lf_count {
            return b"\r\n";
        }

        if self.cr_count > self.lf_count {
            return b"\r";
        }

        b"\n"
    }

    async fn read_bytes_backward<T>(
        &mut self,
        reader: &mut T,
        buf: &mut [u8],
        rewind_after_read: bool,
    ) -> Result<u64>
    where
        T: AsyncRead + AsyncSeek + Unpin,
    {
        let read_len: i64 = buf
            .len()
            .try_into()
            .map_err(|_| anyhow::anyhow!("buffer too large for i64"))?;
        let mut pos = reader.seek(SeekFrom::Current(-read_len)).await?;
        reader.read_exact(buf).await?;
        if !rewind_after_read {
            pos = reader.seek(SeekFrom::Current(-read_len)).await?;
        }
        Ok(pos)
    }

    pub fn final_pos(&self) -> u64 {
        self.final_pos
    }
}

trait ScanStopStrategy {
    fn should_stop(&self, byte: u8, position: usize) -> bool;
}

struct StopAtNonLineEnding;

impl ScanStopStrategy for StopAtNonLineEnding {
    fn should_stop(&self, byte: u8, _: usize) -> bool {
        byte != b'\n' && byte != b'\r'
    }
}

struct StopAtStartOfFile;
impl ScanStopStrategy for StopAtStartOfFile {
    fn should_stop(&self, _: u8, position: usize) -> bool {
        position == 0
    }
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

    let mut line_ending_stats =
        LineEndingDetector::from_reader(&mut file, StopAtNonLineEnding).await?;

    file.seek(tokio::io::SeekFrom::End(0)).await?;
    let pos = line_ending_stats.final_pos();
    if pos == file_size - 1 {
        file.seek(SeekFrom::End(0)).await?;
        line_ending_stats = LineEndingDetector::from_reader(&mut file, StopAtStartOfFile).await?;
        let line_ending = line_ending_stats.dominant_line_ending();
        file.seek(SeekFrom::End(0)).await?;
        file.write_all(line_ending).await?;
    } else if pos == 0 {
        file.set_len(0).await?;
    } else {
        let line_ending = line_ending_stats.dominant_line_ending();
        // Only one line_ending at the end of the file.
        let final_cursor_pos = pos + 1 + line_ending.len() as u64;
        if final_cursor_pos == file_size {
            return Ok((0, Vec::new()));
        }

        file.seek(SeekFrom::Current(1)).await?;
        file.write_all(line_ending).await?;
        file.set_len(pos + 1 + line_ending.len() as u64).await?;
    }
    file.flush().await?;
    file.shutdown().await?;
    Ok((1, format!("Fixing {filename}\n").into_bytes()))
}

#[cfg(test)]
mod tests {
    use super::*;

    use bstr::ByteSlice;
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;
    use tokio::io::BufReader;

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
    async fn test_line_ending_stats_with_various_block_sizes() {
        let cases: &[(&[u8], &[u8])] = &[
            (b"line1\r\nline2\r\n", b"\r\n"),
            (b"line1\nline2\n", b"\n"),
            (b"line1\rline2\r", b"\r"),
            (b"line1\r\nline2\nline3\r\n", b"\r\n"),
            (b"no line endings", b"\n"),
            (b"", b"\n"),
        ];

        let block_sizes = [1, 2, 4, 8, 16, 1024, 4096];

        for &(input, expected) in cases {
            for &block_size in &block_sizes {
                let cursor = Cursor::new(input);
                let mut reader = BufReader::new(cursor);

                let line_ending_detector = LineEndingDetector::from_reader_with_block_size(
                    &mut reader,
                    StopAtNonLineEnding,
                    block_size,
                )
                .await
                .unwrap();

                assert_eq!(
                    line_ending_detector.dominant_line_ending(),
                    expected,
                    "Failed for input {:?} with block size {}",
                    String::from_utf8_lossy(input),
                    block_size
                );
            }
        }
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
