use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use futures::TryStreamExt;
use prek_consts::env_vars::EnvVars;
use reqwest::Certificate;
use tokio::io::{AsyncRead, AsyncWriteExt};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::debug;

use crate::archive::ArchiveExtension;
use crate::checksum::{HashReader, Sha256Digest};
use crate::fs::Simplified;
use crate::store::Store;
use crate::{archive, warn_user};

#[derive(Debug, Clone, Copy, Default)]
pub(crate) enum DownloadVerification {
    #[default]
    None,
    Sha256(Sha256Digest),
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
    verification: DownloadVerification,
) -> Result<TempDownload> {
    download_to_temp_file(url, filename, store, verification, |req| req).await
}

pub(crate) async fn download_and_extract(
    url: &str,
    filename: &str,
    store: &Store,
    verification: DownloadVerification,
) -> Result<TempDownload> {
    download_and_extract_with(url, filename, store, verification, |req| req).await
}

pub(crate) async fn download_and_extract_with(
    url: &str,
    filename: &str,
    store: &Store,
    verification: DownloadVerification,
    customize_request: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> Result<TempDownload> {
    let mut download =
        download_to_temp_file(url, filename, store, verification, customize_request).await?;
    download.path = extract_download(&download, filename).await?;
    Ok(download)
}

async fn extract_download(download: &TempDownload, filename: &str) -> Result<PathBuf> {
    let ext = ArchiveExtension::from_path(filename)?;

    let extract_dir = download.path().with_file_name("extract");
    fs_err::tokio::create_dir_all(&extract_dir).await?;

    let file = fs_err::tokio::File::open(download.path()).await?;
    archive::unpack(file, ext, &extract_dir).await?;

    let extracted = match archive::strip_component(&extract_dir) {
        Ok(top_level) => top_level,
        Err(archive::Error::NonSingularArchive(_)) => extract_dir,
        Err(err) => return Err(err.into()),
    };

    Ok(extracted)
}

async fn download_to_temp_file(
    url: &str,
    filename: &str,
    store: &Store,
    verification: DownloadVerification,
    customize_request: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
) -> Result<TempDownload> {
    let temp_dir = tempfile::tempdir_in(store.scratch_path())?;
    let path = temp_dir.path().join(filename);
    debug!(url = %url, temp_dir = ?temp_dir.path(), "Downloading");

    let response = customize_request(REQWEST_CLIENT.get(url))
        .send()
        .await
        .and_then(reqwest::Response::error_for_status)
        .with_context(|| format!("Failed to download file from {url}"))?;

    let mut stream = response
        .bytes_stream()
        .map_err(std::io::Error::other)
        .into_async_read()
        .compat();

    match verification {
        DownloadVerification::None => {
            write_download(&mut stream, &path, url).await?;
        }
        DownloadVerification::Sha256(required_digest) => {
            let mut reader = HashReader::new(stream);
            write_download(&mut reader, &path, url).await?;
            required_digest.verify(reader.finish(), path.display())?;
        }
    }

    Ok(TempDownload {
        path,
        _temp_dir: temp_dir,
    })
}

async fn write_download(
    reader: &mut (impl AsyncRead + Unpin),
    path: &Path,
    url: &str,
) -> Result<()> {
    let mut file = fs_err::tokio::File::create(path)
        .await
        .with_context(|| format!("Failed to create temporary download `{}`", path.display()))?;
    tokio::io::copy(reader, &mut file)
        .await
        .with_context(|| format!("Failed to download file from {url} to `{}`", path.display()))?;
    file.flush()
        .await
        .with_context(|| format!("Failed to flush temporary download `{}`", path.display()))?;
    Ok(())
}

pub(crate) static REQWEST_CLIENT: LazyLock<reqwest::Client> = LazyLock::new(|| {
    let native_tls = EnvVars::var_as_bool(EnvVars::PREK_NATIVE_TLS).unwrap_or(false);

    let cert_file = EnvVars::var_os(EnvVars::SSL_CERT_FILE).map(PathBuf::from);
    let cert_dirs: Vec<_> = if let Some(cert_dirs) = EnvVars::var_os(EnvVars::SSL_CERT_DIR) {
        std::env::split_paths(&cert_dirs).collect()
    } else {
        vec![]
    };

    let certs = load_certs_from_paths(cert_file.as_deref(), &cert_dirs);
    create_reqwest_client(native_tls, certs)
});

fn load_pem_certs_from_file(path: &Path) -> Result<Vec<Certificate>> {
    let cert_data = fs_err::read(path)?;
    let certs = Certificate::from_pem_bundle(&cert_data)
        .or_else(|_| Certificate::from_pem(&cert_data).map(|cert| vec![cert]))?;
    Ok(certs)
}

/// Load certificate from certificate directory.
fn load_pem_certs_from_dir(dir: &Path) -> Result<Vec<Certificate>> {
    let mut certs = Vec::new();

    for entry in fs_err::read_dir(dir)?.flatten() {
        let path = entry.path();

        // `openssl rehash` used to create this directory uses symlinks. So,
        // make sure we resolve them.
        let metadata = match fs_err::metadata(&path) {
            Ok(metadata) => metadata,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Dangling symlink
                continue;
            }
            Err(_) => {
                continue;
            }
        };

        if metadata.is_file() {
            if let Ok(mut loaded) = load_pem_certs_from_file(&path) {
                certs.append(&mut loaded);
            }
        }
    }

    Ok(certs)
}

fn load_certs_from_paths(file: Option<&Path>, dirs: &[impl AsRef<Path>]) -> Vec<Certificate> {
    let mut certs = Vec::new();

    if let Some(file) = file {
        match load_pem_certs_from_file(file) {
            Ok(mut loaded) => certs.append(&mut loaded),
            Err(e) => {
                warn_user!(
                    "Failed to load certificates from {}: {e}",
                    file.simplified_display().cyan(),
                );
            }
        }
    }

    for dir in dirs {
        match load_pem_certs_from_dir(dir.as_ref()) {
            Ok(mut loaded) => certs.append(&mut loaded),
            Err(e) => {
                warn_user!(
                    "Failed to load certificates from {}: {}",
                    dir.as_ref().simplified_display().cyan(),
                    e
                );
            }
        }
    }

    certs
}

fn create_reqwest_client(native_tls: bool, custom_certs: Vec<Certificate>) -> reqwest::Client {
    let builder =
        reqwest::ClientBuilder::new().user_agent(format!("prek/{}", crate::version::version()));

    let builder = if native_tls {
        debug!("Using native TLS for reqwest client");
        // Use rustls with rustls-platform-verifier which uses the platform's native certificate facilities.
        builder.tls_backend_rustls().tls_certs_merge(custom_certs)
    } else {
        let root_certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
            .iter()
            .filter_map(|cert_der| Certificate::from_der(cert_der).ok());

        // Merge custom certificates on top of webpki-root-certs
        builder
            .tls_backend_rustls()
            .tls_certs_only(custom_certs)
            .tls_certs_merge(root_certs)
    };

    builder.build().expect("Failed to build reqwest client")
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::str::FromStr;

    use anyhow::Result;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::DownloadVerification;
    use crate::checksum::Sha256Digest;
    use crate::store::Store;

    const DATA_SHA256: &str = "3a6eb0790f39ac87c94f3856b2dd2c5d110e6811602261a9a923d3bb23adc8b7";
    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    const TEST_CERT_PEM: &str = "-----BEGIN CERTIFICATE-----
MIIBtjCCAVugAwIBAgITBmyf1XSXNmY/Owua2eiedgPySjAKBggqhkjOPQQDAjA5
MQswCQYDVQQGEwJVUzEPMA0GA1UEChMGQW1hem9uMRkwFwYDVQQDExBBbWF6b24g
Um9vdCBDQSAzMB4XDTE1MDUyNjAwMDAwMFoXDTQwMDUyNjAwMDAwMFowOTELMAkG
A1UEBhMCVVMxDzANBgNVBAoTBkFtYXpvbjEZMBcGA1UEAxMQQW1hem9uIFJvb3Qg
Q0EgMzBZMBMGByqGSM49AgEGCCqGSM49AwEHA0IABCmXp8ZBf8ANm+gBG1bG8lKl
ui2yEujSLtf6ycXYqm0fc4E7O5hrOXwzpcVOho6AF2hiRVd9RFgdszflZwjrZt6j
QjBAMA8GA1UdEwEB/wQFMAMBAf8wDgYDVR0PAQH/BAQDAgGGMB0GA1UdDgQWBBSr
ttvXBp43rDCGB5Fwx5zEGbF4wDAKBggqhkjOPQQDAgNJADBGAiEA4IWSoxe3jfkr
BqWTrBqYaGFy+uGh0PsceGCmQ5nFuMQCIQCcAu/xlJyzlvnrxir4tiz+OpAUFteM
YyRIHN8wfdVoOw==
-----END CERTIFICATE-----\n";

    fn write_cert(path: &Path) {
        fs_err::write(path, TEST_CERT_PEM).expect("failed to write test certificate");
    }

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
    fn test_load_pem_certs_from_file() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let cert_path = temp_dir.path().join("cert.pem");
        write_cert(&cert_path);

        let certs = super::load_pem_certs_from_file(&cert_path)?;
        assert_eq!(certs.len(), 1);

        Ok(())
    }

    #[test]
    fn test_load_pem_certs_from_dir_skips_invalid_files() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let cert_dir = temp_dir.path().join("certs");
        fs_err::create_dir(&cert_dir)?;

        write_cert(&cert_dir.join("valid.pem"));
        fs_err::write(cert_dir.join("invalid.pem"), "not a certificate")?;

        let certs = super::load_pem_certs_from_dir(&cert_dir)?;
        assert_eq!(certs.len(), 1);

        Ok(())
    }

    #[test]
    fn test_load_certs_from_paths_combines_sources() -> Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let cert_file = temp_dir.path().join("cert-file.pem");
        write_cert(&cert_file);

        let cert_dir = temp_dir.path().join("cert-dir");
        fs_err::create_dir(&cert_dir)?;
        write_cert(&cert_dir.join("cert-in-dir.pem"));
        fs_err::write(cert_dir.join("garbage.txt"), "invalid")?;

        let certs = super::load_certs_from_paths(Some(&cert_file), &[&cert_dir]);
        assert_eq!(certs.len(), 2);

        Ok(())
    }

    #[tokio::test]
    async fn test_native_tls() {
        let client = super::create_reqwest_client(true, vec![]);
        let resp = client.get("https://github.com").send().await;
        assert!(resp.is_ok(), "Failed to send request with native TLS");
    }

    #[tokio::test]
    async fn downloads_file_without_checksum() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path()).init()?;
        let (url, server) = serve_once(b"data").await?;

        let download = super::download_to_temp_file(
            &url,
            "archive.tar.gz",
            &store,
            DownloadVerification::None,
            |req| req,
        )
        .await?;

        assert_eq!(fs_err::tokio::read(download.path()).await?, b"data");
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn download_artifact_keeps_plain_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path()).init()?;
        let (url, server) = serve_once(b"data").await?;

        let download = super::download_artifact(
            &url,
            "rustup-init",
            &store,
            DownloadVerification::Sha256(Sha256Digest::from_str(DATA_SHA256)?),
        )
        .await?;

        assert_eq!(
            download.path().file_name().and_then(|name| name.to_str()),
            Some("rustup-init")
        );
        assert_eq!(fs_err::tokio::read(download.path()).await?, b"data");
        assert!(!download.path().with_file_name("extract").exists());
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn download_and_extract_artifact_rejects_unsupported_archive() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path()).init()?;
        let (url, server) = serve_once(b"data").await?;

        let result =
            super::download_and_extract(&url, "archive.rar", &store, DownloadVerification::None)
                .await;

        let Err(err) = result else {
            panic!("expected unsupported archive error");
        };
        assert!(err.to_string().contains("Unsupported archive type"));
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn downloads_verified_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path()).init()?;
        let (url, server) = serve_once(b"data").await?;

        let download = super::download_to_temp_file(
            &url,
            "archive.tar.gz",
            &store,
            DownloadVerification::Sha256(Sha256Digest::from_str(DATA_SHA256)?),
            |req| req,
        )
        .await?;

        assert_eq!(fs_err::tokio::read(download.path()).await?, b"data");
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn downloads_verified_chunked_file() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path()).init()?;
        let (url, server) = serve_chunked(&[b"da", b"ta"]).await?;

        let download = super::download_to_temp_file(
            &url,
            "archive.tar.gz",
            &store,
            DownloadVerification::Sha256(Sha256Digest::from_str(DATA_SHA256)?),
            |req| req,
        )
        .await?;

        assert_eq!(fs_err::tokio::read(download.path()).await?, b"data");
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn rejects_downloaded_file_with_mismatched_checksum() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store = Store::from_path(temp.path()).init()?;
        let (url, server) = serve_once(b"data").await?;

        let result = super::download_to_temp_file(
            &url,
            "archive.tar.gz",
            &store,
            DownloadVerification::Sha256(Sha256Digest::from_str(EMPTY_SHA256)?),
            |req| req,
        )
        .await;

        let Err(err) = result else {
            panic!("expected checksum mismatch");
        };
        assert!(err.to_string().contains("SHA256 checksum mismatch"));
        server.await??;
        Ok(())
    }
}
