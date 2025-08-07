use std::env;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Result, bail};
use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, UpdateRequest};
use semver::Version;
use std::process::Command;
use tokio::task::JoinSet;
use tracing::{debug, enabled, trace, warn};

use constants::env_vars::EnvVars;

use crate::fs::LockedFile;
use crate::process::Cmd;
use crate::store::{CacheBucket, Store};

// The version range of `uv` to check. Should update periodically.
const MIN_UV_VERSION: &str = "0.7.0";
const MAX_UV_VERSION: &str = "0.8.6";

fn get_uv_version(uv_path: &Path) -> Result<Version> {
    let output = Command::new(uv_path)
        .arg("--version")
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to execute uv: {}", e))?;

    if !output.status.success() {
        bail!("Failed to get uv version");
    }

    let version_output = String::from_utf8_lossy(&output.stdout);
    let version_str = version_output
        .split_whitespace()
        .nth(1)
        .ok_or_else(|| anyhow::anyhow!("Invalid version output format"))?;

    Version::parse(version_str).map_err(Into::into)
}

static UV_EXE: LazyLock<Option<(PathBuf, Version)>> = LazyLock::new(|| {
    let min_version = Version::parse(MIN_UV_VERSION).ok()?;
    let max_version = Version::parse(MAX_UV_VERSION).ok()?;

    for uv_path in which::which_all("uv").ok()? {
        debug!("Found uv in PATH: {}", uv_path.display());

        if let Ok(version) = get_uv_version(&uv_path) {
            if max_version >= version && version >= min_version {
                return Some((uv_path, version));
            }
            warn!(
                "Detected system uv version {} — expected a version between {} and {}.",
                version, min_version, max_version
            );
        }
    }

    None
});

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
    /// Download uv from GitHub releases.
    GitHub,
    /// Download uv from `PyPi`.
    PyPi(PyPiMirror),
    /// Install uv by running `pip install uv`.
    Pip,
}

impl InstallSource {
    async fn install(&self, target: &Path) -> Result<()> {
        match self {
            Self::GitHub => self.install_from_github(target).await,
            Self::PyPi(source) => self.install_from_pypi(target, source).await,
            Self::Pip => self.install_from_pip(target).await,
        }
    }

    async fn install_from_github(&self, target: &Path) -> Result<()> {
        let mut installer = AxoUpdater::new_for("uv");
        installer
            .configure_version_specifier(UpdateRequest::SpecificTag(MAX_UV_VERSION.to_string()));
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
            unsafe { env::set_var("INSTALLER_PRINT_VERBOSE", "1") };
        } else {
            installer.disable_installer_output();
        }
        // We don't want the installer to modify the PATH, and don't need the receipt.
        unsafe { env::set_var("UV_UNMANAGED_INSTALL", "1") };

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

    async fn install_from_pypi(&self, target: &Path, _source: &PyPiMirror) -> Result<()> {
        // TODO: Implement this, currently just fallback to pip install
        // Determine the host system
        // Get the html page
        // Parse html, get the latest version url
        // Download the tarball
        // Extract the tarball
        self.install_from_pip(target).await
    }

    async fn install_from_pip(&self, target: &Path) -> Result<()> {
        Cmd::new("python3", "pip install uv")
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--prefix")
            .arg(target)
            .arg(format!("uv=={MAX_UV_VERSION}"))
            .check(true)
            .output()
            .await?;

        let bin_dir = target.join(if cfg!(windows) { "Scripts" } else { "bin" });
        let lib_dir = target.join(if cfg!(windows) { "Lib" } else { "lib" });

        let uv = target
            .join(&bin_dir)
            .join("uv")
            .with_extension(env::consts::EXE_EXTENSION);
        fs_err::tokio::rename(
            &uv,
            target.join("uv").with_extension(env::consts::EXE_EXTENSION),
        )
        .await?;
        fs_err::tokio::remove_dir_all(bin_dir).await?;
        fs_err::tokio::remove_dir_all(lib_dir).await?;

        Ok(())
    }
}

pub(crate) struct Uv {
    path: PathBuf,
}

impl Uv {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub(crate) fn cmd(&self, summary: &str, store: &Store) -> Cmd {
        let mut cmd = Cmd::new(&self.path, summary);
        cmd.env(EnvVars::UV_CACHE_DIR, store.cache_path(CacheBucket::Uv));
        cmd
    }

    async fn select_source() -> Result<InstallSource> {
        async fn check_github(client: &reqwest::Client) -> Result<bool> {
            let url = format!(
                "https://github.com/astral-sh/uv/releases/download/{MAX_UV_VERSION}/uv-x86_64-unknown-linux-gnu.tar.gz"
            );
            let response = client
                .head(url)
                .timeout(Duration::from_secs(3))
                .send()
                .await?;
            trace!(?response, "Checked GitHub");
            Ok(response.status().is_success())
        }

        async fn select_best_pypi(client: &reqwest::Client) -> Result<PyPiMirror> {
            let mut best = PyPiMirror::Pypi;
            let mut tasks = PyPiMirror::iter()
                .map(|source| {
                    let client = client.clone();
                    async move {
                        let url = format!("{}uv/", source.url());
                        let response = client
                            .head(&url)
                            .timeout(Duration::from_secs(2))
                            .send()
                            .await;
                        (source, response)
                    }
                })
                .collect::<JoinSet<_>>();

            while let Some(result) = tasks.join_next().await {
                if let Ok((source, response)) = result {
                    trace!(?source, ?response, "Checked source");
                    if response.is_ok_and(|resp| resp.status().is_success()) {
                        best = source;
                        break;
                    }
                }
            }

            Ok(best)
        }

        let client = reqwest::Client::new();
        let source = tokio::select! {
            Ok(true) = check_github(&client) => InstallSource::GitHub,
            Ok(source) = select_best_pypi(&client) => InstallSource::PyPi(source),
            else => {
                warn!("Failed to check uv source availability, falling back to pip install");
                InstallSource::Pip
            }
        };

        trace!(?source, "Selected uv source");
        Ok(source)
    }

    pub async fn install(uv_dir: &Path) -> Result<Self> {
        // 1) Check if system `uv` meets minimum version requirement
        if let Some((uv_path, version)) = UV_EXE.as_ref() {
            trace!(
                "Using system uv version {} at {}",
                version,
                uv_path.display()
            );
            return Ok(Self::new(uv_path.clone()));
        }

        // 2) Use or install managed `uv`
        let uv_path = uv_dir.join("uv").with_extension(env::consts::EXE_EXTENSION);

        if uv_path.is_file() {
            trace!(uv = %uv_path.display(), "Found managed uv");
            return Ok(Self::new(uv_path));
        }

        // Install new managed uv with proper locking
        fs_err::tokio::create_dir_all(&uv_dir).await?;
        let _lock = LockedFile::acquire(uv_dir.join(".lock"), "uv").await?;

        if uv_path.is_file() {
            trace!(uv = %uv_path.display(), "Found managed uv");
            return Ok(Self::new(uv_path));
        }

        let source = Self::select_source().await?;
        source.install(uv_dir).await?;

        Ok(Self::new(uv_path))
    }
}
