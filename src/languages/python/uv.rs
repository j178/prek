use std::env;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Result, bail};
use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, UpdateRequest};
use semver::Version;
use tokio::task::JoinSet;
use tracing::{debug, enabled, trace, warn};

use constants::env_vars::EnvVars;

use crate::fs::LockedFile;
use crate::process::Cmd;
use crate::store::{CacheBucket, Store};

// The version of `uv` to install. Should update periodically.
const UV_VERSION: &str = "0.8.3";

static UV_EXE: LazyLock<Result<PathBuf, which::Error>> = LazyLock::new(|| {
    which::which("uv").inspect(|uv| {
        debug!("Found uv in PATH: {}", uv.display());
    })
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
            .arg(format!("uv=={UV_VERSION}"))
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
                "https://github.com/astral-sh/uv/releases/download/{UV_VERSION}/uv-x86_64-unknown-linux-gnu.tar.gz"
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

    async fn get_uv_version(uv_path: &Path) -> Result<Version> {
        let output = Cmd::new(uv_path, "Checking uv version")
            .arg("--version")
            .check(false)
            .output()
            .await?;

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

    pub async fn install(uv_dir: &Path) -> Result<Self> {
        // 1) Check if system `uv` meets minimum version requirement
        if let Ok(uv_path) = UV_EXE.as_ref() {
            if let Ok(version) = Self::get_uv_version(uv_path).await {
                let min_version = Version::parse(UV_VERSION)?;

                if version < min_version {
                    warn!(
                        "System uv version {} is older than minimum required version {}, \
                     pre-commit-hooks will install its own version.",
                        version, min_version
                    );
                }
                return Ok(Self::new(uv_path.clone()));
            }
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
