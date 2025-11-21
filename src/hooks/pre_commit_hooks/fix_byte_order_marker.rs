use std::path::Path;

use anyhow::Result;
use futures::StreamExt;
use tokio::io::AsyncReadExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";
const BUFFER_SIZE: usize = 8192; // 8KB buffer for streaming

pub(crate) async fn fix_byte_order_marker(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| fix_file(hook.project().relative_path(), filename).await)
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

async fn fix_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let file_path = file_base.join(filename);

    let mut file = fs_err::tokio::File::open(&file_path).await?;
    let mut bom_buffer = [0u8; 3];

    let bytes_read = file.read(&mut bom_buffer).await?;

    if bytes_read < 3 || bom_buffer != UTF8_BOM {
        return Ok((0, Vec::new()));
    }

    let mut content = Vec::new();
    file.read_to_end(&mut content).await?;
    fs_err::tokio::write(&file_path, &content).await?;

    Ok((
        1,
        format!("{}: removed byte-order marker\n", filename.display()).into_bytes(),
    ))
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
    async fn test_file_with_bom() -> Result<()> {
        let dir = tempdir()?;
        let content = b"\xef\xbb\xbfHello, World!";
        let file_path = create_test_file(&dir, "with_bom.txt", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;

        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("removed byte-order marker"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"Hello, World!");

        Ok(())
    }

    #[tokio::test]
    async fn test_file_without_bom() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Hello, World!";
        let file_path = create_test_file(&dir, "without_bom.txt", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;

        assert_eq!(code, 0);
        assert!(output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.txt", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;

        assert_eq!(code, 0);
        assert!(output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_file_shorter_than_bom() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Hi";
        let file_path = create_test_file(&dir, "short.txt", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;

        assert_eq!(code, 0);
        assert!(output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_file_with_partial_bom() -> Result<()> {
        let dir = tempdir()?;
        let content = b"\xef\xbbHello"; // Only first 2 bytes of BOM
        let file_path = create_test_file(&dir, "partial_bom.txt", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;

        assert_eq!(code, 0);
        assert!(output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_bom_only_file() -> Result<()> {
        let dir = tempdir()?;
        let content = b"\xef\xbb\xbf";
        let file_path = create_test_file(&dir, "bom_only.txt", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;

        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("removed byte-order marker"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_utf8_content_with_bom() -> Result<()> {
        let dir = tempdir()?;
        let content = b"\xef\xbb\xbf\xe4\xb8\xad\xe6\x96\x87"; // BOM + Chinese characters "中文"
        let file_path = create_test_file(&dir, "utf8_with_bom.txt", content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;

        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("removed byte-order marker"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"\xe4\xb8\xad\xe6\x96\x87"); // Just the Chinese characters

        // Verify we can still read it as valid UTF-8
        let text = String::from_utf8(new_content)?;
        assert_eq!(text, "中文");

        Ok(())
    }

    #[tokio::test]
    async fn test_large_file_streaming() -> Result<()> {
        let dir = tempdir()?;

        // Create a large file (>64KB) with BOM
        let mut content = Vec::with_capacity(100_000);
        content.extend_from_slice(b"\xef\xbb\xbf");
        content.extend(b"x".repeat(100_000));

        let file_path = create_test_file(&dir, "large_with_bom.txt", &content).await?;

        let (code, output) = fix_file(Path::new(""), &file_path).await?;

        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("removed byte-order marker"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content.len(), 100_000);
        assert!(new_content.iter().all(|&b| b == b'x'));

        Ok(())
    }
}
