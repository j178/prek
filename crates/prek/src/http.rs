use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures_util::TryStreamExt;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use reqwest::Certificate;
use tokio::io::AsyncWriteExt;
use tokio_util::io::StreamReader;
use tracing::debug;

use crate::checksum::{HashReader, Sha256Digest};
use crate::store::Store;
use crate::warn_user;

#[derive(Debug, Clone, Copy, Eq, PartialEq, Default, strum::AsRefStr, strum::EnumString)]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum DownloadChecksumPolicy {
    #[default]
    WarnMissing,
    Required,
    Disabled,
}

impl DownloadChecksumPolicy {
    pub(crate) fn from_env(env_vars: &impl EnvVarsRead) -> Self {
        match env_vars.var(EnvVars::PREK_DOWNLOAD_CHECKSUM_POLICY) {
            Ok(value) => Self::from_str(&value).unwrap_or_else(|_| {
                warn_user!(
                    "Invalid value for {}: {:?}. Expected warn-missing, required, or disabled; using default ({:?})",
                    EnvVars::PREK_DOWNLOAD_CHECKSUM_POLICY,
                    value,
                    Self::default().as_ref(),
                );
                Self::default()
            }),
            Err(_) => Self::default(),
        }
    }
}

pub(crate) struct TempDownload {
    path: PathBuf,
    _temp_dir: tempfile::TempDir,
}

impl TempDownload {
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }
}

pub(crate) async fn download_artifact(
    url: &str,
    filename: &str,
    store: &Store,
    fetch_checksum: impl AsyncFn() -> Result<Option<Sha256Digest>>,
) -> Result<TempDownload> {
    download_artifact_with(
        url,
        filename,
        store,
        DownloadChecksumPolicy::from_env(&EnvVars),
        fetch_checksum,
        |req| req,
    )
    .await
}

pub(crate) async fn download_artifact_with(
    url: &str,
    filename: &str,
    store: &Store,
    checksum_policy: DownloadChecksumPolicy,
    fetch_checksum: impl AsyncFn() -> Result<Option<Sha256Digest>>,
    customize_request: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> Result<TempDownload> {
    if checksum_policy == DownloadChecksumPolicy::Disabled {
        let (download, _computed_digest) =
            download_to_temp_file(url, filename, store, customize_request).await?;
        return Ok(download);
    }

    let (expected_digest, (download, computed_digest)) = tokio::try_join!(
        fetch_checksum(),
        download_to_temp_file(url, filename, store, customize_request)
    )?;

    if let Some(expected_digest) = expected_digest {
        expected_digest.verify(computed_digest, download.path().display())?;
    } else if checksum_policy == DownloadChecksumPolicy::WarnMissing {
        warn_user!(
            "No checksum found for `{filename}`; skipping checksum verification. \
             Set `{}=required` to make missing checksums a hard error.",
            EnvVars::PREK_DOWNLOAD_CHECKSUM_POLICY
        );
    } else {
        bail!("Checksum verification is required for `{filename}`, but no checksum was found");
    }

    Ok(download)
}

async fn download_to_temp_file(
    url: &str,
    filename: &str,
    store: &Store,
    customize_request: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> Result<(TempDownload, Sha256Digest)> {
    let temp_dir = tempfile::tempdir_in(store.scratch_path())?;
    let path = temp_dir.path().join(filename);

    debug!(%url, "Downloading");

    let response = customize_request(reqwest_client()?.get(url))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Failed to download file from {url}"))?;

    let stream = response.bytes_stream().map_err(std::io::Error::other);

    let mut reader = HashReader::new(StreamReader::new(stream));
    let mut file = fs_err::tokio::File::create(&path)
        .await
        .with_context(|| format!("Failed to create temporary download `{}`", path.display()))?;
    tokio::io::copy(&mut reader, &mut file)
        .await
        .with_context(|| format!("Failed to download file from {url} to `{}`", path.display()))?;
    file.flush()
        .await
        .with_context(|| format!("Failed to flush temporary download `{}`", path.display()))?;
    let computed_digest = reader.finish();

    debug!(%filename, "Downloaded");

    Ok((
        TempDownload {
            path,
            _temp_dir: temp_dir,
        },
        computed_digest,
    ))
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum ClientError {
    #[error("Failed to initialize HTTP client")]
    Build(#[source] reqwest::Error),

    #[error(
        "Failed to initialize HTTP client with system certificates. Ensure system CA certificates are installed, or set `PREK_SYSTEM_CERTS=false` to use bundled certificates"
    )]
    SystemCertificates(#[source] reqwest::Error),
}

pub(crate) fn reqwest_client() -> Result<&'static reqwest::Client, &'static ClientError> {
    static CLIENT: LazyLock<Result<reqwest::Client, ClientError>> =
        LazyLock::new(reqwest_client_from_env);

    CLIENT.as_ref()
}

fn reqwest_client_from_env() -> Result<reqwest::Client, ClientError> {
    // TODO: Remove PREK_NATIVE_TLS compatibility in the next breaking release.
    let system_certs_var = if EnvVars.is_set(EnvVars::PREK_SYSTEM_CERTS) {
        EnvVars::PREK_SYSTEM_CERTS
    } else {
        EnvVars::PREK_NATIVE_TLS
    };
    let system_certs = EnvVars
        .var_as_bool(system_certs_var)
        .unwrap_or_else(|value| {
            warn_user!(
                "Invalid value for {}: {:?}. Expected a boolean value; using default ({:?})",
                system_certs_var,
                value,
                "false",
            );
            Some(false)
        })
        .unwrap_or(false);

    // An explicitly configured source replaces default roots even when empty.
    let certs = ssl_certs::load_certs_from_env().map(|certs| {
        for error in certs.errors {
            warn_user!("Failed to load certificates: {error}");
        }
        certs
            .certs
            .iter()
            .filter_map(|cert| Certificate::from_der(cert).ok())
            .collect()
    });
    create_reqwest_client(system_certs, certs)
}

// Bound the connect phase, and fail a download that stalls (no bytes received for this long)
// rather than blocking forever. This is an inactivity timeout, so it doesn't penalize slow but
// progressing downloads, and it lets callers fall back to another source.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

fn create_reqwest_client(
    system_certs: bool,
    custom_certs: Option<Vec<Certificate>>,
) -> Result<reqwest::Client, ClientError> {
    let system_certs = system_certs && custom_certs.is_none();
    let builder = reqwest::ClientBuilder::new()
        .tls_backend_rustls()
        .user_agent(format!("prek/{}", crate::version::version()))
        .connect_timeout(CONNECT_TIMEOUT)
        .read_timeout(READ_TIMEOUT);

    let builder = if let Some(custom_certs) = custom_certs {
        debug!("Using custom certificates for reqwest client");
        builder.tls_certs_only(custom_certs)
    } else if system_certs {
        debug!("Using system certificates for reqwest client");
        // With no explicit roots, reqwest uses rustls-platform-verifier.
        builder
    } else {
        debug!("Using bundled certificates for reqwest client");
        let root_certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .filter_map(|cert_der| Certificate::from_der(cert_der).ok());

        builder.tls_certs_only(root_certs)
    };

    builder.build().map_err(|error| {
        if system_certs {
            ClientError::SystemCertificates(error)
        } else {
            ClientError::Build(error)
        }
    })
}

#[cfg(test)]
mod tests {
    use std::str::FromStr;

    use anyhow::Result;
    use prek_consts::env_vars::EnvVars;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use crate::checksum::Sha256Digest;
    use crate::store::Store;

    use super::DownloadChecksumPolicy;

    const DATA_SHA256: &str = "3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7";
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    async fn serve_once(body: &'static [u8]) -> Result<(String, JoinHandle<Result<()>>)> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await?;
            stream
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .as_bytes(),
                )
                .await?;
            stream.write_all(body).await?;
            Ok(())
        });

        Ok((url, handle))
    }

    async fn serve_chunked(
        chunks: &'static [&'static [u8]],
    ) -> Result<(String, JoinHandle<Result<()>>)> {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
        let url = format!("http://{}", listener.local_addr()?);
        let handle = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await?;
            let mut request = [0_u8; 1024];
            let _ = stream.read(&mut request).await?;
            stream
                .write_all(
                    b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n",
                )
                .await?;
            for chunk in chunks {
                stream
                    .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                    .await?;
                stream.write_all(chunk).await?;
                stream.write_all(b"\r\n").await?;
                stream.flush().await?;
                tokio::task::yield_now().await;
            }
            stream.write_all(b"0\r\n\r\n").await?;
            Ok(())
        });

        Ok((url, handle))
    }

    #[test]
    fn download_checksum_policy_reads_env() {
        assert_eq!(
            DownloadChecksumPolicy::from_env(&EnvVars::from_map(&[])),
            DownloadChecksumPolicy::WarnMissing
        );
        assert_eq!(
            DownloadChecksumPolicy::from_env(&EnvVars::from_map(&[(
                EnvVars::PREK_DOWNLOAD_CHECKSUM_POLICY,
                "required",
            )])),
            DownloadChecksumPolicy::Required
        );
        assert_eq!(
            DownloadChecksumPolicy::from_env(&EnvVars::from_map(&[(
                EnvVars::PREK_DOWNLOAD_CHECKSUM_POLICY,
                "disabled",
            )])),
            DownloadChecksumPolicy::Disabled
        );
        assert_eq!(
            DownloadChecksumPolicy::from_env(&EnvVars::from_map(&[(
                EnvVars::PREK_DOWNLOAD_CHECKSUM_POLICY,
                "always",
            )])),
            DownloadChecksumPolicy::WarnMissing
        );
    }

    /// Default roots trust public HTTPS. Unrelated or empty custom roots replace
    /// them and fail during certificate verification.
    #[tokio::test]
    async fn test_tls_certificates() -> Result<()> {
        let custom_cert = reqwest::Certificate::from_pem(indoc::indoc! {b"
            -----BEGIN CERTIFICATE-----
            MIIBdDCCARqgAwIBAgIUZ1kmigC8aj49VPno0N1d7vKCDbswCgYIKoZIzj0EAwIw
            FzEVMBMGA1UEAwwMcHJlayB0ZXN0IENBMCAXDTI2MTAyNjExNTM1OFoYDzIxMjYx
            MDAyMTE1MzU4WjAXMRUwEwYDVQQDDAxwcmVrIHRlc3QgQ0EwWTATBgcqhkjOPQIB
            BggqhkjOPQMBBwNCAATnHgUgx6Q0+Oeri/poUmyQLJiXk09eceAySysTGV+SI5wc
            dxLzmCZvsolKlQ4A66QRleU3emn0GBKgnz2uqxFYo0IwQDAdBgNVHQ4EFgQU53Go
            +jl4xGYHpgHFQpoYrttAbsQwDwYDVR0TAQH/BAUwAwEB/zAOBgNVHQ8BAf8EBAMC
            AQYwCgYIKoZIzj0EAwIDSAAwRQIhALvrJ3U2Yk1k+iLH+rjj9jSnpt3bv5bhhKxK
            WMKctiPPAiAcPHp1Si2wO726t4qJK5lX9cnTC3Azy/ZASjAzf8Dx1g==
            -----END CERTIFICATE-----
        "})?;

        for system_certs in [false, true] {
            let client = super::create_reqwest_client(system_certs, None)?;
            client.get("https://example.com").send().await?;

            for certs in [vec![], vec![custom_cert.clone()]] {
                let client = super::create_reqwest_client(system_certs, Some(certs))?;
                let Err(error) = client.get("https://example.com").send().await else {
                    anyhow::bail!("Custom certificates must replace default roots");
                };
                assert!(
                    format!("{error:?}").contains("UnknownIssuer"),
                    "Expected an untrusted issuer with system_certs={system_certs}: {error:?}"
                );
            }
        }
        Ok(())
    }

    #[tokio::test]
    async fn downloads_file_without_checksum() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path())?.init()?;
        let (url, server) = serve_once(b"data").await?;

        let (download, _computed_digest) =
            super::download_to_temp_file(&url, "archive.tar.gz", &store, |req| req).await?;

        assert_eq!(fs_err::tokio::read(download.path()).await?, b"data");
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn download_artifact_keeps_plain_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path())?.init()?;
        let (url, server) = serve_once(b"data").await?;

        let download = super::download_artifact(&url, "rustup-init", &store, async || {
            Ok(Some(Sha256Digest::from_str(DATA_SHA256)?))
        })
        .await?;

        assert_eq!(
            download.path().file_name().and_then(|name| name.to_str()),
            Some("rustup-init")
        );
        assert_eq!(fs_err::tokio::read(download.path()).await?, b"data");
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn download_artifact_rejects_mismatched_checksum() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path())?.init()?;
        let (url, server) = serve_once(b"data").await?;

        let result = super::download_artifact(&url, "archive.tar.gz", &store, async || {
            Ok(Some(Sha256Digest::from_str(EMPTY_SHA256)?))
        })
        .await;

        let Err(err) = result else {
            panic!("expected checksum mismatch");
        };
        assert!(err.to_string().contains("SHA256 checksum mismatch"));
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn downloads_file_and_computes_checksum() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path())?.init()?;
        let (url, server) = serve_once(b"data").await?;

        let (download, sha256_digest) =
            super::download_to_temp_file(&url, "archive.tar.gz", &store, |req| req).await?;

        assert_eq!(fs_err::tokio::read(download.path()).await?, b"data");
        assert_eq!(sha256_digest, Sha256Digest::from_str(DATA_SHA256)?);
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn downloads_chunked_file_and_computes_checksum() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path())?.init()?;
        let (url, server) = serve_chunked(&[b"da", b"ta"]).await?;

        let (download, sha256_digest) =
            super::download_to_temp_file(&url, "archive.tar.gz", &store, |req| req).await?;

        assert_eq!(fs_err::tokio::read(download.path()).await?, b"data");
        assert_eq!(sha256_digest, Sha256Digest::from_str(DATA_SHA256)?);
        server.await??;
        Ok(())
    }
}
