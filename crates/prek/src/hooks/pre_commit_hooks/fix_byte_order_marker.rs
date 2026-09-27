use std::io::{ErrorKind, Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::Result;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{FixArgs, parse_hook_args, run_blocking_file_checks};

const UTF8_BOM: &[u8] = b"\xef\xbb\xbf";

/// Runs the `fix-byte-order-marker` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: FixArgs = parse_hook_args(hook)?;
    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        move |file_path, filename| fix_file(file_path, filename, args.check),
    )
    .await
}

fn fix_file(file_path: &Path, filename: &Path, check: bool) -> Result<HookOutput> {
    let needs_fix = fix_file_sync(file_path, check)?;
    if !needs_fix {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    let action = if check { "would remove" } else { "removed" };
    Ok(HookOutput::known(
        1,
        format!("{}: {action} byte-order marker\n", filename.display()).into_bytes(),
        !check,
    ))
}

fn fix_file_sync(file_path: &Path, check: bool) -> Result<bool> {
    let mut file = fs_err::OpenOptions::new()
        .read(true)
        .write(!check)
        .open(file_path)?;

    let mut bom_buffer = [0u8; 3];
    match file.read_exact(&mut bom_buffer) {
        Ok(()) => {}
        Err(err) if err.kind() == ErrorKind::UnexpectedEof => return Ok(false),
        Err(err) => return Err(err.into()),
    }
    if bom_buffer != UTF8_BOM {
        return Ok(false);
    }
    if check {
        return Ok(true);
    }

    // Shift the payload in place so large files do not need a full second buffer.
    let file_len = file.seek(SeekFrom::End(0))?;
    let mut buf = [0u8; 8192];
    let mut read_pos = UTF8_BOM.len() as u64;
    while read_pos < file_len {
        let chunk_len = usize::try_from((file_len - read_pos).min(buf.len() as u64))?;
        file.seek(SeekFrom::Start(read_pos))?;
        file.read_exact(&mut buf[..chunk_len])?;
        file.seek(SeekFrom::Start(read_pos - UTF8_BOM.len() as u64))?;
        file.write_all(&buf[..chunk_len])?;
        read_pos += chunk_len as u64;
    }
    file.set_len(file_len - UTF8_BOM.len() as u64)?;
    Ok(true)
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

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 1);
        let output_str = String::from_utf8_lossy(&result.output);
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

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 0);
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

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_file_shorter_than_bom() -> Result<()> {
        let dir = tempdir()?;
        let content = b"Hi";
        let file_path = create_test_file(&dir, "short.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_file_with_partial_bom() -> Result<()> {
        let dir = tempdir()?;
        let content = b"\xef\xbbHello"; // Only first 2 bytes of BOM
        let file_path = create_test_file(&dir, "partial_bom.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_bom_only_file() -> Result<()> {
        let dir = tempdir()?;
        let content = b"\xef\xbb\xbf";
        let file_path = create_test_file(&dir, "bom_only.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 1);
        let output_str = String::from_utf8_lossy(&result.output);
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

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 1);
        let output_str = String::from_utf8_lossy(&result.output);
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

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 1);
        let output_str = String::from_utf8_lossy(&result.output);
        assert!(output_str.contains("removed byte-order marker"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content.len(), 100_000);
        assert!(new_content.iter().all(|&b| b == b'x'));

        Ok(())
    }
}
