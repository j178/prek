use std::path::Path;

use anyhow::Result;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

pub(crate) async fn check_symlinks(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(|filename| check_file(hook.project().relative_path(), filename))
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

#[allow(clippy::unused_async)]
async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let path = file_base.join(filename);

    // Check if it's a symlink and if it's broken
    if path.is_symlink() && !path.exists() {
        let error_message = format!("{}: Broken symlink\n", filename.display());
        return Ok((1, error_message.into_bytes()));
    }

    Ok((0, Vec::new()))
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
    async fn test_regular_file() -> Result<()> {
        let dir = tempdir()?;
        let content = b"regular file content";
        let file_path = create_test_file(&dir, "regular.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_valid_symlink_unix() -> Result<()> {
        let dir = tempdir()?;
        let target = create_test_file(&dir, "target.txt", b"content").await?;
        let link_path = dir.path().join("link.txt");
        tokio::fs::symlink(&target, &link_path).await?;

        let (code, output) = check_file(Path::new(""), &link_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_broken_symlink_unix() -> Result<()> {
        let dir = tempdir()?;
        let link_path = dir.path().join("broken_link.txt");
        let nonexistent = dir.path().join("nonexistent.txt");
        tokio::fs::symlink(&nonexistent, &link_path).await?;

        let (code, output) = check_file(Path::new(""), &link_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Broken symlink"));
        Ok(())
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn test_valid_symlink_windows() -> Result<()> {
        let dir = tempdir()?;
        let target = create_test_file(&dir, "target.txt", b"content").await?;
        let link_path = dir.path().join("link.txt");

        // Windows requires different APIs for file vs directory symlinks
        tokio::fs::symlink_file(&target, &link_path).await?;

        let (code, output) = check_file(Path::new(""), &link_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    #[cfg(windows)]
    async fn test_broken_symlink_windows() -> Result<()> {
        let dir = tempdir()?;
        let link_path = dir.path().join("broken_link.txt");
        let nonexistent = dir.path().join("nonexistent.txt");

        // On Windows, symlink creation might require admin privileges
        // If this fails in CI, the test will be skipped
        if tokio::fs::symlink_file(&nonexistent, &link_path)
            .await
            .is_err()
        {
            // Skipping test: insufficient permissions for symlink creation on Windows
            return Ok(());
        }

        let (code, output) = check_file(Path::new(""), &link_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Broken symlink"));
        Ok(())
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_valid_symlink_macos() -> Result<()> {
        let dir = tempdir()?;
        let target = create_test_file(&dir, "target.txt", b"content").await?;
        let link_path = dir.path().join("link.txt");
        tokio::fs::symlink(&target, &link_path).await?;

        let (code, output) = check_file(Path::new(""), &link_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    #[cfg(target_os = "macos")]
    async fn test_broken_symlink_macos() -> Result<()> {
        let dir = tempdir()?;
        let link_path = dir.path().join("broken_link.txt");
        let nonexistent = dir.path().join("nonexistent.txt");
        tokio::fs::symlink(&nonexistent, &link_path).await?;

        let (code, output) = check_file(Path::new(""), &link_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Broken symlink"));
        Ok(())
    }
}
