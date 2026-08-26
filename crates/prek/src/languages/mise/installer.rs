use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use semver::Version;
use target_lexicon::{Architecture, ArmArchitecture, Environment, HOST, OperatingSystem, Triple};
use tracing::{debug, trace, warn};

use super::{inherited_mise_vars, mise_ceiling};
use crate::archive;
use crate::checksum::{Sha256Digest, digest_from_sha256sums};
use crate::fs::{LockedFile, is_executable};
use crate::git;
use crate::http::{REQWEST_CLIENT, download_artifact};
use crate::languages::version::{
    SemverRequest, ToolchainPolicy, ToolchainSource, find_system_executables,
};
use crate::process::Cmd;
use crate::store::Store;

// This is the first release where MISE_CEILING_PATHS also isolates early .miserc discovery.
const MIN_MISE_VERSION: Version = Version::new(2026, 5, 18);

static MISE_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    EnvVars
        .var(EnvVars::PREK_INTERNAL__MISE_BINARY_NAME)
        .unwrap_or_else(|_| "mise".to_string())
});

#[derive(Debug)]
pub(crate) struct MiseResult {
    mise: PathBuf,
    version: Version,
}

impl Display for MiseResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.mise.display(), self.version)
    }
}

impl MiseResult {
    fn from_dir(dir: &Path, version: Version) -> Self {
        Self {
            mise: bin_dir(dir).join("mise").with_extension(EXE_EXTENSION),
            version,
        }
    }

    pub(crate) async fn from_executable(mise: PathBuf) -> Result<Self> {
        let isolated = tempfile::tempdir()?;
        let mut command = Cmd::new(&mise);
        for key in inherited_mise_vars() {
            command.env_remove(key);
        }
        // Even `mise --version` discovers miserc files, initializes backend state,
        // runs migrations and cache pruning, then checks for updates. CI disables
        // the update check; disposable roots keep the probe away from user state.
        let output = command
            .current_dir(isolated.path())
            .env(EnvVars::CI, "1")
            .env(EnvVars::MISE_DATA_DIR, isolated.path().join("data"))
            .env(EnvVars::MISE_CACHE_DIR, isolated.path().join("cache"))
            .env(EnvVars::MISE_CONFIG_DIR, isolated.path().join("config"))
            .env(EnvVars::MISE_STATE_DIR, isolated.path().join("state"))
            .env(
                EnvVars::MISE_SYSTEM_CONFIG_DIR,
                isolated.path().join("system-config"),
            )
            .env(
                EnvVars::MISE_SYSTEM_DATA_DIR,
                isolated.path().join("system-data"),
            )
            .env(EnvVars::MISE_CEILING_PATHS, mise_ceiling(isolated.path())?)
            .env(EnvVars::MISE_NO_CONFIG, "1")
            .arg("--version")
            .check(true)
            .output()
            .await?;
        let output = String::from_utf8_lossy(&output.stdout);
        let version = output
            .split_whitespace()
            .next()
            .context("Failed to parse mise version output")?
            .parse()
            .context("Failed to parse mise version")?;

        Ok(Self { mise, version })
    }

    pub(crate) fn mise(&self) -> &Path {
        &self.mise
    }

    pub(crate) fn version(&self) -> &Version {
        &self.version
    }
}

pub(crate) struct MiseInstaller {
    root: PathBuf,
}

impl MiseInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) async fn install(
        &self,
        store: &Store,
        request: &SemverRequest,
        policy: ToolchainPolicy,
    ) -> Result<MiseResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;
        let _lock = LockedFile::acquire(self.root.join(".lock"), "mise").await?;

        for &source in policy.search_order() {
            let result = match source {
                ToolchainSource::Managed => self.find_installed(request).await,
                ToolchainSource::System => self.find_system_mise(request).await,
            };
            if let Some(result) = result {
                trace!(%result, ?source, "Found mise");
                return Ok(result);
            }
        }

        if !policy.allows_download() {
            anyhow::bail!("No compatible mise executable found for toolchain policy: {policy}");
        }

        let version = self.resolve_version(request).await?;
        trace!(%version, "Downloading mise");
        self.download(store, &version).await
    }

    async fn find_installed(&self, request: &SemverRequest) -> Option<MiseResult> {
        let installed = fs_err::read_dir(&self.root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(err) => {
                    warn!(?err, "Failed to read managed mise entry");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .filter_map(|entry| {
                let version = entry
                    .file_name()
                    .to_string_lossy()
                    .parse::<Version>()
                    .ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| b.cmp(a));

        for (version, path) in installed {
            let candidate = MiseResult::from_dir(&path, version.clone());
            if !is_compatible(request, &version) || !is_executable(candidate.mise()) {
                continue;
            }
            match MiseResult::from_executable(candidate.mise().to_path_buf()).await {
                Ok(result) if result.version() == &version => return Some(result),
                Ok(result) => {
                    warn!(expected = %version, found = %result.version(), path = %path.display(), "Managed mise version mismatch");
                }
                Err(err) => {
                    warn!(?err, path = %path.display(), "Failed to query managed mise version");
                }
            }
        }

        None
    }

    async fn find_system_mise(&self, request: &SemverRequest) -> Option<MiseResult> {
        let paths = match find_system_executables(&*MISE_BINARY_NAME, &self.root) {
            Ok(paths) => paths,
            Err(err) => {
                debug!(%err, "No mise executable found in PATH");
                return None;
            }
        };

        for path in paths {
            match MiseResult::from_executable(path).await {
                Ok(result) if is_compatible(request, result.version()) => return Some(result),
                Ok(result) => trace!(%result, "System mise does not match request"),
                Err(err) => warn!(?err, "Failed to query system mise version"),
            }
        }

        None
    }

    async fn resolve_version(&self, request: &SemverRequest) -> Result<Version> {
        let output = git::git_cmd()?
            .arg("ls-remote")
            .arg("--tags")
            .arg("https://github.com/jdx/mise")
            .output()
            .await?;
        let output = str::from_utf8(&output.stdout)?;

        output
            .lines()
            .filter_map(|line| {
                let reference = line.split_once('\t')?.1;
                reference
                    .strip_prefix("refs/tags/v")?
                    .parse::<Version>()
                    .ok()
            })
            .filter(|version| is_compatible(request, version))
            .max()
            .context("No released mise version matches the request")
    }

    async fn download(&self, store: &Store, version: &Version) -> Result<MiseResult> {
        let (platform, extension) = release_platform(&HOST)?;
        let filename = format!("mise-v{version}-{platform}.{extension}");
        let base_url = format!("https://github.com/jdx/mise/releases/download/v{version}");
        let url = format!("{base_url}/{filename}");
        let checksum_url = format!("{base_url}/SHASUMS256.txt");

        let download = download_artifact(&url, &filename, store, async || {
            Self::fetch_checksum(&checksum_url, &filename).await
        })
        .await
        .context("Failed to download mise")?;
        let install_dir = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&self.root)?;
        let target_bin_dir = bin_dir(install_dir.path());
        fs_err::tokio::create_dir_all(&target_bin_dir).await?;
        let target_binary = target_bin_dir.join("mise").with_extension(EXE_EXTENSION);
        if HOST.operating_system == OperatingSystem::Windows {
            fs_err::tokio::copy(download.path(), &target_binary).await?;
        } else {
            let extracted = archive::extract_archive(download.path())
                .await
                .context("Failed to extract mise")?;
            let source = bin_dir(&extracted)
                .join("mise")
                .with_extension(EXE_EXTENSION);
            fs_err::tokio::rename(&source, &target_binary).await?;
        }
        crate::fs::make_executable(&target_binary)?;

        let target = self.root.join(version.to_string());
        if target.exists() {
            fs_err::tokio::remove_dir_all(&target).await?;
        }
        fs_err::tokio::rename(install_dir.keep(), &target).await?;

        Ok(MiseResult::from_dir(&target, version.clone()))
    }

    async fn fetch_checksum(url: &str, filename: &str) -> Result<Option<Sha256Digest>> {
        let checksums = REQWEST_CLIENT
            .get(url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch mise checksums from {url}"))?
            .error_for_status()
            .with_context(|| format!("Failed to fetch mise checksums from {url}"))?
            .text()
            .await?;
        digest_from_sha256sums(&checksums, filename)
    }
}

fn is_compatible(request: &SemverRequest, version: &Version) -> bool {
    is_supported_version(version) && request.matches(version)
}

pub(crate) fn is_supported_version(version: &Version) -> bool {
    version >= &MIN_MISE_VERSION
}

fn release_platform(host: &Triple) -> Result<(String, &'static str)> {
    let platform = match (host.operating_system, host.architecture) {
        (OperatingSystem::Darwin(_), Architecture::X86_64) => "macos-x64",
        (OperatingSystem::Darwin(_), Architecture::Aarch64(_)) => "macos-arm64",
        (OperatingSystem::Linux, Architecture::X86_64) => "linux-x64",
        (OperatingSystem::Linux, Architecture::Aarch64(_)) => "linux-arm64",
        (OperatingSystem::Linux, Architecture::Arm(ArmArchitecture::Armv7)) => "linux-armv7",
        (OperatingSystem::Windows, Architecture::X86_64) => "windows-x64",
        (OperatingSystem::Windows, Architecture::Aarch64(_)) => "windows-arm64",
        (operating_system, architecture) => anyhow::bail!(
            "Unsupported platform for mise: operating_system={operating_system:?}, architecture={architecture:?}"
        ),
    };
    let extension = if host.operating_system == OperatingSystem::Windows {
        // mise's signed ZIP starts with a zipsign preamble, which async_zip's streaming reader
        // cannot skip.
        "exe"
    } else {
        "tar.gz"
    };

    let platform = if host.operating_system == OperatingSystem::Linux
        && matches!(
            host.environment,
            Environment::Musl
                | Environment::Musleabi
                | Environment::Musleabihf
                | Environment::Muslabi64
        ) {
        format!("{platform}-musl")
    } else {
        platform.to_string()
    };

    Ok((platform, extension))
}

fn bin_dir(prefix: &Path) -> PathBuf {
    prefix.join("bin")
}
