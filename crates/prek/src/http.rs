use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use futures::TryStreamExt;
use prek_consts::env_vars::EnvVars;
use reqwest::Certificate;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::debug;

use crate::archive::ArchiveExtension;
use crate::fs::Simplified;
use crate::store::Store;
use crate::{archive, warn_user};

pub(crate) async fn download_and_extract(
    url: &str,
    filename: &str,
    store: &Store,
    callback: impl AsyncFn(&Path) -> Result<()>,
) -> Result<()> {
    download_and_extract_with(url, filename, store, |req| req, callback).await
}

/// Like [`download_and_extract`], but accepts a `customize_request` closure
/// that can modify the [`reqwest::RequestBuilder`] before it is sent (e.g. to
/// add authentication headers).
pub(crate) async fn download_and_extract_with(
    url: &str,
    filename: &str,
    store: &Store,
    customize_request: impl FnOnce(reqwest::RequestBuilder) -> reqwest::RequestBuilder,
    callback: impl AsyncFn(&Path) -> Result<()>,
) -> Result<()> {
    let response = customize_request(REQWEST_CLIENT.get(url))
        .send()
        .await
        .with_context(|| format!("Failed to download file from {url}"))?;
    if !response.status().is_success() {
        anyhow::bail!(
            "Failed to download file from {}: {}",
            url,
            response.status()
        );
    }

    let tarball = response
        .bytes_stream()
        .map_err(std::io::Error::other)
        .into_async_read()
        .compat();

    let scratch_dir = store.scratch_path();
    let temp_dir = tempfile::tempdir_in(&scratch_dir)?;
    debug!(url = %url, temp_dir = ?temp_dir.path(), "Downloading");

    let ext = ArchiveExtension::from_path(filename)?;
    archive::unpack(tarball, ext, temp_dir.path()).await?;

    let extracted = match archive::strip_component(temp_dir.path()) {
        Ok(top_level) => top_level,
        Err(archive::Error::NonSingularArchive(_)) => temp_dir.path().to_path_buf(),
        Err(err) => return Err(err.into()),
    };

    callback(&extracted).await?;

    drop(temp_dir);

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
    use anyhow::Result;
    use std::path::Path;

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
}
