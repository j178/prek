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
    let response = REQWEST_CLIENT
        .get(url)
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

    let root_certs = webpki_root_certs::TLS_SERVER_ROOT_CERTS
        .iter()
        .filter_map(|cert_der| Certificate::from_der(cert_der).ok());

    let builder = if native_tls {
        debug!("Using native TLS for reqwest client");
        builder.tls_backend_native().tls_certs_merge(custom_certs)
    } else {
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
    #[tokio::test]
    async fn test_native_tls() {
        let client = super::create_reqwest_client(true, vec![]);
        let resp = client.get("https://github.com").send().await;
        assert!(resp.is_ok(), "Failed to send request with native TLS");
    }
}
