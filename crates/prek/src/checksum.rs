use std::fmt;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result, bail};
use aws_lc_rs::digest::{Context as Sha256Context, SHA256};
use tokio::io::AsyncReadExt;

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct Sha256Digest([u8; 32]);

impl Sha256Digest {
    pub(crate) async fn verify_file(self, path: &Path) -> Result<()> {
        let actual = Self::from_file(path).await?;
        if actual != self {
            bail!(
                "SHA256 checksum mismatch for `{}`: expected {self}, got {actual}",
                path.display()
            );
        }
        Ok(())
    }

    async fn from_file(path: &Path) -> Result<Self> {
        let mut file = fs_err::tokio::File::open(path).await.with_context(|| {
            format!(
                "Failed to open `{}` for SHA256 verification",
                path.display()
            )
        })?;
        let mut hasher = Sha256Context::new(&SHA256);
        let mut buf = [0_u8; 8192];

        loop {
            let read = file.read(&mut buf).await.with_context(|| {
                format!(
                    "Failed to read `{}` for SHA256 verification",
                    path.display()
                )
            })?;
            if read == 0 {
                break;
            }
            Sha256Context::update(&mut hasher, &buf[..read]);
        }

        let digest = hasher.finish();
        let mut result = [0_u8; 32];
        result.copy_from_slice(digest.as_ref());
        Ok(Self(result))
    }
}

impl FromStr for Sha256Digest {
    type Err = anyhow::Error;

    fn from_str(value: &str) -> Result<Self> {
        let value = value.strip_prefix("sha256:").unwrap_or(value).trim();
        if value.len() != 64 {
            bail!("SHA256 digest must be 64 hex characters");
        }

        let mut digest = [0_u8; 32];
        hex::decode_to_slice(value, &mut digest).context("Failed to parse SHA256 digest")?;
        Ok(Self(digest))
    }
}

impl fmt::Display for Sha256Digest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}

pub(crate) fn digest_from_sha256sums(contents: &str, filename: &str) -> Result<Sha256Digest> {
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((digest, name)) = line.split_once(char::is_whitespace) else {
            continue;
        };
        let name = name.trim().trim_start_matches('*');
        if name == filename {
            return digest.parse();
        }
    }

    bail!("No SHA256 digest found for `{filename}`");
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn parses_plain_and_prefixed_digest() {
        let plain = Sha256Digest::from_str(EMPTY_SHA256).unwrap();
        let prefixed = Sha256Digest::from_str(&format!("sha256:{EMPTY_SHA256}")).unwrap();

        assert_eq!(plain, prefixed);
        assert_eq!(plain.to_string(), EMPTY_SHA256);
    }

    #[test]
    fn rejects_invalid_digest() {
        assert!(Sha256Digest::from_str("abc").is_err());
        assert!(
            Sha256Digest::from_str(
                "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
            )
            .is_err()
        );
    }

    #[test]
    fn parses_sha256sums_file() -> Result<()> {
        let digest = digest_from_sha256sums(
            indoc::indoc! {"
                0000000000000000000000000000000000000000000000000000000000000000  other.tar.gz
                e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *target.tar.gz
            "},
            "target.tar.gz",
        )?;

        assert_eq!(digest.to_string(), EMPTY_SHA256);
        Ok(())
    }

    #[tokio::test]
    async fn verifies_file_contents() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let file = temp.path().join("empty");
        fs_err::write(&file, [])?;

        Sha256Digest::from_str(EMPTY_SHA256)?
            .verify_file(&file)
            .await?;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_file_mismatch() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let file = temp.path().join("not-empty");
        fs_err::write(&file, b"data")?;

        let err = Sha256Digest::from_str(EMPTY_SHA256)?
            .verify_file(&file)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("SHA256 checksum mismatch"));
        Ok(())
    }
}
