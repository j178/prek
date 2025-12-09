use std::path::Path;

use anyhow::Result;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

const BLACKLIST: &[&[u8]] = &[
    b"BEGIN RSA PRIVATE KEY",
    b"BEGIN DSA PRIVATE KEY",
    b"BEGIN EC PRIVATE KEY",
    b"BEGIN OPENSSH PRIVATE KEY",
    b"BEGIN PRIVATE KEY",
    b"PuTTY-User-Key-File-2",
    b"BEGIN SSH2 ENCRYPTED PRIVATE KEY",
    b"BEGIN PGP PRIVATE KEY BLOCK",
    b"BEGIN ENCRYPTED PRIVATE KEY",
    b"BEGIN OpenVPN Static key V1",
];

pub(crate) async fn detect_private_key(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
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

async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let content = fs_err::tokio::read(file_base.join(filename)).await?;

    // Use memchr's memmem for faster substring search
    for pattern in BLACKLIST {
        if memchr::memmem::find(&content, pattern).is_some() {
            let error_message = format!("Private key found: {}\n", filename.display());
            return Ok((1, error_message.into_bytes()));
        }
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
    async fn test_no_private_key() -> Result<()> {
        let dir = tempdir()?;
        let content = b"This is just a regular file\nwith some content\n";
        let file_path = create_test_file(&dir, "clean.txt", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_rsa_private_key() -> Result<()> {
        let dir = tempdir()?;
        let content = b"-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY-----\n";
        let file_path = create_test_file(&dir, "id_rsa", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Private key found"));
        assert!(output_str.contains("id_rsa"));
        Ok(())
    }

    #[tokio::test]
    async fn test_key_in_middle_of_file() -> Result<()> {
        let dir = tempdir()?;
        let content =
            b"Some documentation\n\nHere is a key:\n-----BEGIN RSA PRIVATE KEY-----\ndata\n";
        let file_path = create_test_file(&dir, "doc.txt", content).await?;
        let (code, _output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        Ok(())
    }

    #[tokio::test]
    async fn test_false_positive_similar_text() -> Result<()> {
        let dir = tempdir()?;
        let content = b"This file talks about BEGIN_RSA_PRIVATE_KEY but doesn't contain one\n";
        let file_path = create_test_file(&dir, "false_positive.txt", content).await?;
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
    async fn test_binary_file_with_key() -> Result<()> {
        let dir = tempdir()?;
        let mut content = vec![0xFF, 0xFE, 0x00];
        content.extend_from_slice(b"BEGIN RSA PRIVATE KEY");
        let file_path = create_test_file(&dir, "binary.dat", &content).await?;
        let (code, _output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        Ok(())
    }
}
