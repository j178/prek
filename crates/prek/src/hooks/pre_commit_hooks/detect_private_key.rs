use std::path::Path;
use std::sync::LazyLock;

use aho_corasick::AhoCorasick;
use anyhow::Result;
use tokio::io::AsyncReadExt;

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
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
const BUFFER_SIZE: usize = 8192;

// Keep at most the longest marker minus one byte so split matches can span two reads.
const CARRY_CAPACITY: usize = {
    let mut max_len = 0;
    let mut idx = 0;
    while idx < BLACKLIST.len() {
        let len = BLACKLIST[idx].len();
        if len > max_len {
            max_len = len;
        }
        idx += 1;
    }

    max_len.saturating_sub(1)
};
static PRIVATE_KEY_MATCHER: LazyLock<AhoCorasick> = LazyLock::new(|| {
    AhoCorasick::new(BLACKLIST).expect("private key blacklist patterns should be valid")
});

pub(crate) async fn detect_private_key(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    run_concurrent_file_checks(filenames.iter().copied(), *CONCURRENCY, |filename| {
        check_file(hook.project().relative_path(), filename)
    })
    .await
}

/// Scan the file in chunks while preserving a small tail between reads.
///
/// For example, if one read ends with `BEGIN RSA PRIV` and the next read starts
/// with `ATE KEY`, we keep the tail of the first read, prepend it to the second
/// read, and search the combined window so `BEGIN RSA PRIVATE KEY` is still found.
async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let mut file = fs_err::tokio::File::open(file_base.join(filename)).await?;
    let mut buf = vec![0u8; BUFFER_SIZE + CARRY_CAPACITY];
    let mut carry_len = 0;

    loop {
        let bytes_read = file.read(&mut buf[carry_len..]).await?;
        if bytes_read == 0 {
            break;
        }

        let search_len = carry_len + bytes_read;
        let search_buf = &buf[..search_len];

        if PRIVATE_KEY_MATCHER.find(search_buf).is_some() {
            let error_message = format!("Private key found: {}\n", filename.display());
            return Ok((1, error_message.into_bytes()));
        }

        // Move the tail of this chunk to the front of the buffer so a key marker
        // split across this read and the next read is still seen.
        carry_len = CARRY_CAPACITY.min(search_len);
        if carry_len > 0 {
            buf.copy_within(search_len - carry_len..search_len, 0);
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
