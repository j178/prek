use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Result, bail};
use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, UpdateRequest};
use futures::StreamExt;
use semver::Version;
use std::process::Command;
use target_lexicon::{Architecture, ArmArchitecture, HOST, OperatingSystem};
use tokio::task::JoinSet;
use tracing::{debug, enabled, trace, warn};

use constants::env_vars::EnvVars;

use crate::fs::LockedFile;
use crate::process::Cmd;
use crate::store::{CacheBucket, Store};
use crate::{archive, version};

// The version range of `uv` we will install. Should update periodically.
const CUR_UV_VERSION: &str = "0.8.6";
const UV_VERSION_RANGE: &str = ">=0.7.0, <0.9.0";

fn get_platform_tag() -> Result<String> {
    let platform_tag = match (HOST.operating_system, HOST.architecture) {
        // Linux platforms
        // TODO: support musllinux?
        (OperatingSystem::Linux, Architecture::X86_64) => {
            "manylinux_2_17_x86_64.manylinux2014_x86_64"
        }
        (OperatingSystem::Linux, Architecture::Aarch64(_)) => {
            "manylinux_2_17_aarch64.manylinux2014_aarch64.musllinux_1_1_aarch64"
        }
        (OperatingSystem::Linux, Architecture::Arm(ArmArchitecture::Armv7)) => {
            "manylinux_2_17_armv7l.manylinux2014_armv7l"
        } // ARMv7
        (OperatingSystem::Linux, Architecture::Arm(ArmArchitecture::Armv6)) => "linux_armv6l", // Raspberry Pi Zero/1
        (OperatingSystem::Linux, Architecture::X86_32(_)) => {
            "manylinux_2_17_i686.manylinux2014_i686"
        }
        (OperatingSystem::Linux, Architecture::Powerpc64) => {
            "manylinux_2_17_ppc64.manylinux2014_ppc64"
        }
        (OperatingSystem::Linux, Architecture::Powerpc64le) => {
            "manylinux_2_17_ppc64le.manylinux2014_ppc64le"
        }
        (OperatingSystem::Linux, Architecture::S390x) => "manylinux_2_17_s390x.manylinux2014_s390x",
        (OperatingSystem::Linux, Architecture::Riscv64(_)) => "manylinux_2_31_riscv64",

        // macOS platforms
        (OperatingSystem::Darwin(_), Architecture::X86_64) => "macosx_10_12_x86_64",
        (OperatingSystem::Darwin(_), Architecture::Aarch64(_)) => "macosx_11_0_arm64",

        // Windows platforms
        (OperatingSystem::Windows, Architecture::X86_64) => "win_amd64",
        (OperatingSystem::Windows, Architecture::X86_32(_)) => "win32",
        (OperatingSystem::Windows, Architecture::Aarch64(_)) => "win_arm64",

        _ => bail!("Unsupported platform: {HOST}"),
    };

    Ok(platform_tag.to_string())
}

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
    let version_range = semver::VersionReq::parse(UV_VERSION_RANGE).ok()?;

    for uv_path in which::which_all("uv").ok()? {
        debug!("Found uv in PATH: {}", uv_path.display());

        if let Ok(version) = get_uv_version(&uv_path) {
            if version_range.matches(&version) {
                return Some((uv_path, version));
            }
            warn!(
                "Skip system uv version `{}` — expected a version range: `{}`.",
                version, version_range
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
            .configure_version_specifier(UpdateRequest::SpecificTag(CUR_UV_VERSION.to_string()));
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
            unsafe { std::env::set_var("INSTALLER_PRINT_VERBOSE", "1") };
        } else {
            installer.disable_installer_output();
        }
        // We don't want the installer to modify the PATH, and don't need the receipt.
        unsafe { std::env::set_var("UV_UNMANAGED_INSTALL", "1") };

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

    async fn install_from_pypi(&self, target: &Path, source: &PyPiMirror) -> Result<()> {
        let platform_tag = get_platform_tag()?;
        let wheel_name = format!("uv-{CUR_UV_VERSION}-py3-none-{platform_tag}.whl");

        // Use PyPI JSON API instead of parsing HTML
        let client = reqwest::Client::new();
        let api_url = match source {
            PyPiMirror::Pypi => format!("https://pypi.org/pypi/uv/{CUR_UV_VERSION}/json"),
            // For mirrors, we'll fall back to simple API approach
            _ => return self.install_from_simple_api(target, source).await,
        };

        debug!("Fetching uv metadata from: {}", api_url);
        let response = client
            .get(&api_url)
            .header("User-Agent", format!("prek/{}", version::version().version))
            .header("Accept", "*/*")
            .send()
            .await?;

        if !response.status().is_success() {
            bail!(
                "Failed to fetch uv metadata from PyPI: {}",
                response.status()
            );
        }

        let metadata: serde_json::Value = response.json().await?;
        let files = metadata["urls"]
            .as_array()
            .ok_or_else(|| anyhow::anyhow!("Invalid PyPI response: missing urls"))?;

        let wheel_file = files
            .iter()
            .find(|file| {
                file["filename"].as_str() == Some(&wheel_name)
                    && file["packagetype"].as_str() == Some("bdist_wheel")
                    && file["yanked"].as_bool() != Some(true)
            })
            .ok_or_else(|| {
                anyhow::anyhow!("Could not find wheel for {} in PyPI response", wheel_name)
            })?;

        let download_url = wheel_file["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing download URL in PyPI response"))?;

        debug!("Downloading uv wheel: {download_url}");

        // Download and extract the wheel
        self.download_and_extract_wheel(target, download_url).await
    }

    async fn install_from_simple_api(&self, target: &Path, source: &PyPiMirror) -> Result<()> {
        // Fallback for mirrors that don't support JSON API
        let platform_tag = get_platform_tag()?;
        let wheel_name = format!("uv-{CUR_UV_VERSION}-py3-none-{platform_tag}.whl");

        let simple_url = format!("{}uv/", source.url());
        let client = reqwest::Client::new();

        debug!("Fetching from simple API: {}", simple_url);
        let response = client
            .get(&simple_url)
            .header("User-Agent", format!("prek/{}", version::version().version))
            .header("Accept", "*/*")
            .send()
            .await?;
        let html = response.text().await?;

        // Simple string search to find the wheel download link
        let search_pattern = r#"href=""#.to_string();

        let download_path = html
            .lines()
            .find(|line| line.contains(&wheel_name))
            .and_then(|line| {
                if let Some(start) = line.find(&search_pattern) {
                    let start = start + search_pattern.len();
                    if let Some(end) = line[start..].find('"') {
                        return Some(&line[start..start + end]);
                    }
                }
                None
            })
            .ok_or_else(|| {
                anyhow::anyhow!(
                    "Could not find wheel download link for {} in simple API response",
                    wheel_name
                )
            })?;

        // Resolve relative URLs
        let download_url = if download_path.starts_with("http") {
            download_path.to_string()
        } else {
            format!("{simple_url}{download_path}")
        };

        debug!("Downloading uv wheel: {download_url}");
        self.download_and_extract_wheel(target, &download_url).await
    }

    async fn download_and_extract_wheel(&self, target: &Path, download_url: &str) -> Result<()> {
        let client = reqwest::Client::new();
        let response = client
            .get(download_url)
            .header("User-Agent", format!("prek/{}", version::version().version))
            .header("Accept", "*/*")
            .send()
            .await?;

        if !response.status().is_success() {
            bail!("Failed to download wheel: {}", response.status());
        }

        debug!("Downloaded wheel, extracting...");

        // Create a temporary directory to extract the wheel
        let temp_dir = tempfile::tempdir()?;
        let temp_extract_dir = temp_dir.path();

        // Extract the wheel using the existing archive functionality
        let stream = response.bytes_stream();
        let reader = tokio_util::io::StreamReader::new(
            stream.map(|result| result.map_err(std::io::Error::other)),
        );

        // TODO: check sha256 checksum
        archive::unzip(reader, temp_extract_dir).await?;

        // Find the uv binary in the extracted contents
        let data_dir = format!("uv-{CUR_UV_VERSION}.data");
        let extracted_uv = temp_extract_dir
            .join(data_dir)
            .join("scripts")
            .join("uv")
            .with_extension(EXE_EXTENSION);

        // Copy the binary to the target location
        let target_path = target.join("uv").with_extension(EXE_EXTENSION);
        fs_err::tokio::copy(&extracted_uv, &target_path).await?;

        // Set executable permissions on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let metadata = fs_err::tokio::metadata(&target_path).await?;
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            fs_err::tokio::set_permissions(&target_path, perms).await?;
        }

        debug!("Extracted uv binary to: {}", target_path.display());
        Ok(())
    }

    async fn install_from_pip(&self, target: &Path) -> Result<()> {
        // When running `pip install` in multiple threads, it can fail
        // without extracting files properly.
        Cmd::new("python3", "pip install uv")
            .arg("-m")
            .arg("pip")
            .arg("install")
            .arg("--prefix")
            .arg(target)
            .arg(format!("uv=={CUR_UV_VERSION}"))
            .check(true)
            .status()
            .await?;

        let bin_dir = target.join(if cfg!(windows) { "Scripts" } else { "bin" });
        let lib_dir = target.join(if cfg!(windows) { "Lib" } else { "lib" });

        let uv = target
            .join(&bin_dir)
            .join("uv")
            .with_extension(std::env::consts::EXE_EXTENSION);
        fs_err::tokio::rename(
            &uv,
            target
                .join("uv")
                .with_extension(std::env::consts::EXE_EXTENSION),
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
                "https://github.com/astral-sh/uv/releases/download/{CUR_UV_VERSION}/uv-x86_64-unknown-linux-gnu.tar.gz"
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
                            .header("User-Agent", format!("prek/{}", version::version().version))
                            .header("Accept", "*/*")
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
                    if let Ok(resp) = response
                        && resp.status().is_success()
                    {
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

    pub(crate) async fn install(uv_dir: &Path) -> Result<Self> {
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
        let uv_path = uv_dir
            .join("uv")
            .with_extension(std::env::consts::EXE_EXTENSION);

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
