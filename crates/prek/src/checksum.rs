use std::fmt;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::task::{Context as TaskContext, Poll};

use anyhow::{Context, Result, bail};
use aws_lc_rs::digest::{Context as Sha256Context, SHA256};
use tokio::io::{AsyncRead, AsyncReadExt, ReadBuf};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct Sha256Digest([u8; 32]);

pub(crate) struct Sha256Hasher(Sha256Context);

impl Sha256Hasher {
    pub(crate) fn new() -> Self {
        Self(Sha256Context::new(&SHA256))
    }

    pub(crate) fn update(&mut self, data: &[u8]) {
        Sha256Context::update(&mut self.0, data);
    }

    pub(crate) fn finish(self) -> Sha256Digest {
        let digest = self.0.finish();
        Sha256Digest::from_bytes(digest.as_ref())
    }
}

pub(crate) struct HashReader<R> {
    inner: R,
    hasher: Sha256Hasher,
}

impl<R> HashReader<R> {
    pub(crate) fn new(inner: R) -> Self {
        Self {
            inner,
            hasher: Sha256Hasher::new(),
        }
    }

    pub(crate) fn finish(self) -> Sha256Digest {
        self.hasher.finish()
    }
}

impl<R: AsyncRead + Unpin> AsyncRead for HashReader<R> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut TaskContext<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let this = self.get_mut();
        let filled_before = buf.filled().len();

        match Pin::new(&mut this.inner).poll_read(cx, buf) {
            Poll::Ready(Ok(())) => {
                let filled = buf.filled();
                if filled.len() > filled_before {
                    this.hasher.update(&filled[filled_before..]);
                }
                Poll::Ready(Ok(()))
            }
            other => other,
        }
    }
}

impl Sha256Digest {
    pub(crate) fn verify(self, actual: Self, subject: impl fmt::Display) -> Result<()> {
        if actual != self {
            bail!("SHA256 checksum mismatch for `{subject}`: expected {self}, got {actual}");
        }
        Ok(())
    }

    pub(crate) async fn verify_file(self, path: &Path) -> Result<()> {
        let actual = Self::from_file(path).await?;
        self.verify(actual, path.display())
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
        Ok(Self::from_bytes(digest.as_ref()))
    }

    fn from_bytes(bytes: &[u8]) -> Self {
        let mut result = [0_u8; 32];
        result.copy_from_slice(bytes);
        Self(result)
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
        let name = name.trim();
        // GNU-style checksum files may prefix binary-mode filenames with `*`.
        let name = name.strip_prefix('*').unwrap_or(name);
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

    #[test]
    fn parses_sha256sums_binary_mode_marker() -> Result<()> {
        let digest = digest_from_sha256sums(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855 *target.tar.gz",
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
