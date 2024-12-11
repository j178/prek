use std::env;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, UpdateRequest};
use tokio::task::JoinSet;
use tracing::{debug, enabled, trace, warn};

use crate::fs::LockedFile;
use crate::store::Store;

// The version of `uv` to install. Should update periodically.
const UV_VERSION: &str = "0.5.8";

#[derive(Debug)]
enum PyPiMirror {
    Pypi,
    Tuna,
    Aliyun,
    Tencent,
    Custom(String),
}

// TODO: support reading pypi source user config, or allow user to set mirror
// TODO: allow opt-out uv

impl PyPiMirror {
    fn url(&self) -> &str {
        match self {
            Self::Pypi => "https://pypi.org/simple/",
            Self::Tuna => "https://pypi.tuna.tsinghua.edu.cn/simple/",
            Self::Aliyun => "https://mirrors.aliyun.com/pypi/simple/",
            Self::Tencent => "https://mirrors.cloud.tencent.com/pypi/simple/",
            Self::Custom(url) => url,
        }
    }

    fn iter() -> impl Iterator<Item = Self> {
        vec![Self::Pypi, Self::Tuna, Self::Aliyun, Self::Tencent].into_iter()
    }
}

#[derive(Debug)]
enum InstallSource {
    GitHub,
    PyPi(PyPiMirror),
}

impl InstallSource {
    async fn install(&self, target: &Path) -> Result<()> {
        match self {
            Self::GitHub => self.install_from_github(target).await,
            Self::PyPi(source) => self.install_from_pypi(target, source).await,
        }
    }

    async fn install_from_github(&self, target: &Path) -> Result<()> {
        let mut installer = AxoUpdater::new_for("uv");
        installer.configure_version_specifier(UpdateRequest::SpecificTag(UV_VERSION.to_string()));
        installer.always_update(true);
        installer.set_install_dir(&target.to_string_lossy());
        installer.set_release_source(ReleaseSource {
            release_type: ReleaseSourceType::GitHub,
            owner: "astral-sh".to_string(),
            name: "uv".to_string(),
            app_name: "uv".to_string(),
        });
        if enabled!(tracing::Level::DEBUG) {
            installer.enable_installer_output();
            env::set_var("INSTALLER_PRINT_VERBOSE", "1");
        } else {
            installer.disable_installer_output();
        }
        // We don't want the installer to modify the PATH, and don't need the receipt.
        env::set_var("UV_UNMANAGED_INSTALL", "1");

        match installer.run().await {
            Ok(Some(result)) => {
                debug!(
                    uv = %target.display(),
                    version = result.new_version_tag,
                    "Successfully installed uv"
                );
                Ok(())
            }
            Ok(None) => Ok(()),
            Err(err) => {
                warn!(?err, "Failed to install uv");
                Err(err.into())
            }
        }
    }

    async fn install_from_pypi(&self, _target: &Path, _source: &PyPiMirror) -> Result<()> {
        unimplemented!()
    }
}

pub struct UvInstaller;

impl UvInstaller {
    async fn select_source() -> Result<InstallSource> {
        async fn check_github() -> Result<bool> {
            let url = format!("https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-x86_64-unknown-linux-gnu.tar.gz");
            let response = reqwest::Client::new()
                .head(url)
                .timeout(Duration::from_secs(3))
                .send()
                .await?;
            Ok(response.status().is_success())
        }

        async fn best_pypi() -> Result<PyPiMirror> {
            let mut best = PyPiMirror::Pypi;
            let mut tasks = PyPiMirror::iter()
                .map(|source| async move {
                    let url = format!("{}uv/", source.url());
                    let response = reqwest::Client::new()
                        .head(&url)
                        .timeout(Duration::from_secs(2))
                        .send()
                        .await;
                    (source, response)
                })
                .collect::<JoinSet<_>>();

            while let Some(result) = tasks.join_next().await {
                if let Ok((source, response)) = result {
                    trace!(?source, ?response, "Checked source");
                    if response.is_ok_and(|resp|resp.status().is_success()) {
                        best = source;
                        break;
                    }
                }
            }

            Ok(best)
        }

        let source = tokio::select! {
            Ok(true) = check_github() => InstallSource::GitHub,
            Ok(source) = best_pypi() => InstallSource::PyPi(source),
            else => {
                warn!("Failed to check uv source availability, defaulting to GitHub");
                InstallSource::GitHub
            }
        };

        trace!(?source, "Selected uv source");
        Ok(source)
    }

    pub async fn install() -> Result<PathBuf> {
        // 1) Check if `uv` is installed already.
        if let Ok(uv) = which::which("uv") {
            trace!(uv = %uv.display(), "Found uv from PATH");
            return Ok(uv);
        }

        // 2) Check if `uv` is installed by `prefligit`
        let store = Store::from_settings()?;

        let uv_dir = store.uv_path();
        let uv = uv_dir.join("uv").with_extension(env::consts::EXE_EXTENSION);
        if uv.is_file() {
            trace!(uv = %uv.display(), "Found managed uv");
            return Ok(uv);
        }

        fs_err::create_dir_all(&uv_dir)?;
        let _lock = LockedFile::acquire(uv_dir.join(".lock"), "uv").await?;

        if uv.is_file() {
            trace!(uv = %uv.display(), "Found managed uv");
            return Ok(uv);
        }

        let source = Self::select_source().await?;
        source.install(&uv_dir).await?;

        Ok(uv)
    }
}
