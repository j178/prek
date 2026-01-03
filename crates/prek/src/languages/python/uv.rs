use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use http::header::ACCEPT;
use semver::{Version, VersionReq};
use target_lexicon::{Architecture, ArmArchitecture, HOST, OperatingSystem};
use tokio::task::JoinSet;
use tracing::{debug, trace, warn};

use prek_consts::env_vars::EnvVars;

use crate::fs::LockedFile;
use crate::languages::{REQWEST_CLIENT, download_and_extract};
use crate::process::Cmd;
use crate::store::{CacheBucket, Store};
use crate::version;

// The version range of `uv` we will install. Should update periodically.
const CUR_UV_VERSION: &str = "0.9.18";
static UV_VERSION_RANGE: LazyLock<VersionReq> =
    LazyLock::new(|| VersionReq::parse(">=0.7.0, <0.10.0").unwrap());

// Get the uv wheel platform tag for the current host.
fn get_wheel_platform_tag() -> Result<String> {
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
        .map_err(|e| anyhow::anyhow!("Failed to execute uv: {e}"))?;

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
    for uv_path in which::which_all("uv").ok()? {
        debug!("Found uv in PATH: {}", uv_path.display());

        if let Ok(version) = get_uv_version(&uv_path) {
            if UV_VERSION_RANGE.matches(&version) {
                return Some((uv_path, version));
            }
            warn!(
                "Skip system uv version `{version}` — expected a version range: `{}`",
                &*UV_VERSION_RANGE
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
    async fn install(&self, store: &Store, target: &Path) -> Result<()> {
        match self {
            Self::GitHub => self.install_from_github(store, target).await,
            Self::PyPi(source) => self.install_from_pypi(store, target, source).await,
            Self::Pip => self.install_from_pip(target).await,
        }
    }

    async fn install_from_github(&self, store: &Store, target: &Path) -> Result<()> {
        let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
        let archive_name = format!("uv-{HOST}.{ext}");
        let download_url = format!(
            "https://github.com/astral-sh/uv/releases/download/{CUR_UV_VERSION}/{archive_name}"
        );

        download_and_extract(&download_url, &archive_name, store, async |extracted| {
            let source = extracted.join("uv").with_extension(EXE_EXTENSION);
            let target_path = target.join("uv").with_extension(EXE_EXTENSION);

            if target_path.exists() {
                debug!(target = %target.display(), "Removing existing uv");
                fs_err::tokio::remove_dir_all(&target).await?;
            }

            debug!(?source, target = %target_path.display(), "Moving uv to target");
            // TODO: retry on Windows
            fs_err::tokio::rename(source, target_path).await?;

            anyhow::Ok(())
        })
        .await
        .context("Failed to download and extra uv")?;

        Ok(())
    }

    async fn install_from_pypi(
        &self,
        store: &Store,
        target: &Path,
        source: &PyPiMirror,
    ) -> Result<()> {
        let platform_tag = get_wheel_platform_tag()?;
        let wheel_name = format!("uv-{CUR_UV_VERSION}-py3-none-{platform_tag}.whl");

        // Use PyPI JSON API instead of parsing HTML
        let api_url = match source {
            PyPiMirror::Pypi => format!("https://pypi.org/pypi/uv/{CUR_UV_VERSION}/json"),
            // For mirrors, we'll fall back to simple API approach
            _ => return self.install_from_simple_api(store, target, source).await,
        };

        debug!("Fetching uv metadata from: {}", api_url);
        let response = REQWEST_CLIENT
            .get(&api_url)
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
                anyhow::anyhow!("Could not find wheel for {wheel_name} in PyPI response")
            })?;

        let download_url = wheel_file["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing download URL in PyPI response"))?;

        self.download_and_extract_wheel(store, target, &wheel_name, download_url)
            .await
    }

    async fn install_from_simple_api(
        &self,
        store: &Store,
        target: &Path,
        source: &PyPiMirror,
    ) -> Result<()> {
        // Fallback for mirrors that don't support JSON API
        let platform_tag = get_wheel_platform_tag()?;
        let wheel_name = format!("uv-{CUR_UV_VERSION}-py3-none-{platform_tag}.whl");

        let simple_url = format!("{}uv/", source.url());

        debug!("Fetching from simple API: {}", simple_url);
        let response = REQWEST_CLIENT
            .get(&simple_url)
            .header(ACCEPT, "*/*")
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
                    "Could not find wheel download link for {wheel_name} in simple API response"
                )
            })?;

        // Resolve relative URLs
        let download_url = if download_path.starts_with("http") {
            download_path.to_string()
        } else {
            format!("{simple_url}{download_path}")
        };

        self.download_and_extract_wheel(store, target, &wheel_name, &download_url)
            .await
    }

    async fn download_and_extract_wheel(
        &self,
        store: &Store,
        target: &Path,
        filename: &str,
        download_url: &str,
    ) -> Result<()> {
        download_and_extract(download_url, filename, store, async |extracted| {
            // Find the uv binary in the extracted contents
            let data_dir = format!("uv-{CUR_UV_VERSION}.data");
            let extracted_uv = extracted
                .join(data_dir)
                .join("scripts")
                .join("uv")
                .with_extension(EXE_EXTENSION);

            // Copy the binary to the target location
            let target_path = target.join("uv").with_extension(EXE_EXTENSION);
            if target_path.exists() {
                debug!(target = %target.display(), "Removing existing uv");
                fs_err::tokio::remove_dir_all(&target).await?;
            }

            debug!(?extracted_uv, target = %target_path.display(), "Moving uv to target");
            fs_err::tokio::rename(&extracted_uv, &target_path).await?;

            // Set executable permissions on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let metadata = fs_err::tokio::metadata(&target_path).await?;
                let mut perms = metadata.permissions();
                perms.set_mode(0o755);
                fs_err::tokio::set_permissions(&target_path, perms).await?;
            }

            Ok(())
        })
        .await
        .context("Failed to download and extract uv wheel")?;

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
            .arg("--only-binary=:all:")
            .arg("--progress-bar=off")
            .arg("--disable-pip-version-check")
            .arg(format!("uv=={CUR_UV_VERSION}"))
            .check(true)
            .output()
            .await?;

        let local_dir = target.join("local");
        let uv_src = if local_dir.is_dir() {
            &local_dir
        } else {
            target
        };

        let bin_dir = uv_src.join(if cfg!(windows) { "Scripts" } else { "bin" });
        let lib_dir = uv_src.join(if cfg!(windows) { "Lib" } else { "lib" });

        let uv = uv_src
            .join(&bin_dir)
            .join("uv")
            .with_extension(EXE_EXTENSION);
        fs_err::tokio::rename(&uv, target.join("uv").with_extension(EXE_EXTENSION)).await?;
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
        async fn check_github() -> Result<bool> {
            let url = format!(
                "https://github.com/astral-sh/uv/releases/download/{CUR_UV_VERSION}/uv-x86_64-unknown-linux-gnu.tar.gz"
            );
            let response = REQWEST_CLIENT
                .head(url)
                .timeout(Duration::from_secs(3))
                .send()
                .await?;
            trace!(?response, "Checked GitHub");
            Ok(response.status().is_success())
        }

        async fn select_best_pypi() -> Result<PyPiMirror> {
            let mut best = PyPiMirror::Pypi;
            let mut tasks = PyPiMirror::iter()
                .map(|source| {
                    let client = REQWEST_CLIENT.clone();
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

        let source = tokio::select! {
                Ok(true) = check_github() => InstallSource::GitHub,
                Ok(source) = select_best_pypi() => InstallSource::PyPi(source),
                else => {
                    warn!("Failed to check uv source availability, falling back to pip install");
                    InstallSource::Pip
                }

        };

        trace!(?source, "Selected uv source");
        Ok(source)
    }

    pub(crate) async fn install(store: &Store, uv_dir: &Path) -> Result<Self> {
        // 1) Check `uv` alongside `prek` binary (e.g. `uv tool install prek --with uv`)
        let prek_exe = std::env::current_exe()?.canonicalize()?;
        if let Some(prek_dir) = prek_exe.parent() {
            let uv_path = prek_dir.join("uv").with_extension(EXE_EXTENSION);
            if uv_path.is_file() {
                if let Ok(version) = get_uv_version(&uv_path) {
                    if UV_VERSION_RANGE.matches(&version) {
                        trace!(uv = %uv_path.display(), "Found compatible uv alongside prek binary");
                        return Ok(Self::new(uv_path));
                    }
                    warn!(
                        "Skip uv version `{version}` alongside prek binary — expected a version range: `{}`",
                        &*UV_VERSION_RANGE
                    );
                }
            }
        }

        // 2) Check if system `uv` meets minimum version requirement
        if let Some((uv_path, version)) = UV_EXE.as_ref() {
            trace!(
                "Using system uv version {} at {}",
                version,
                uv_path.display()
            );
            return Ok(Self::new(uv_path.clone()));
        }

        // 3) Use or install managed `uv`
        let uv_path = uv_dir.join("uv").with_extension(EXE_EXTENSION);

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

        let source = if let Some(uv_source) = uv_source_from_env() {
            uv_source
        } else {
            Self::select_source().await?
        };
        source.install(store, uv_dir).await?;

        Ok(Self::new(uv_path))
    }
}

fn uv_source_from_env() -> Option<InstallSource> {
    let var = EnvVars::var(EnvVars::PREK_UV_SOURCE).ok()?;
    match var.as_str() {
        "github" => Some(InstallSource::GitHub),
        "pypi" => Some(InstallSource::PyPi(PyPiMirror::Pypi)),
        "tuna" => Some(InstallSource::PyPi(PyPiMirror::Tuna)),
        "aliyun" => Some(InstallSource::PyPi(PyPiMirror::Aliyun)),
        "tencent" => Some(InstallSource::PyPi(PyPiMirror::Tencent)),
        "pip" => Some(InstallSource::Pip),
        custom if custom.starts_with("http") => Some(InstallSource::PyPi(PyPiMirror::Custom(var))),
        _ => {
            warn!("Invalid UV_SOURCE value: {}", var);
            None
        }
    }
}

#[test]
fn ensure_cur_uv_version_in_range() {
    let version = Version::parse(CUR_UV_VERSION).expect("Invalid CUR_UV_VERSION");
    assert!(
        UV_VERSION_RANGE.matches(&version),
        "CUR_UV_VERSION {CUR_UV_VERSION} does not satisfy the version requirement {}",
        &*UV_VERSION_RANGE
    );
}
