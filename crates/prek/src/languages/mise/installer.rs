use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use asyncband::once::OnceMap;
use itertools::Itertools;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use rustc_hash::FxBuildHasher;
use semver::Version;
use target_lexicon::{Architecture, ArmArchitecture, Environment, HOST, OperatingSystem, Triple};
use tracing::{debug, trace, warn};

use super::{inherited_mise_vars, mise_ceiling};
use crate::archive;
use crate::checksum::{digest_from_sha256sums, fetch_checksum};
use crate::fs::{LockedFile, is_executable};
use crate::git;
use crate::http::download_artifact;
use crate::languages::version::{
    SemverRequest, ToolchainPolicy, ToolchainSource, find_system_executables,
    installed_version_dirs,
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
    pub(crate) async fn from_executable(mise: PathBuf) -> Result<Self> {
        static CACHE: LazyLock<OnceMap<PathBuf, Version, FxBuildHasher>> =
            LazyLock::new(|| OnceMap::with_hasher(FxBuildHasher));

        let key = fs_err::canonicalize(&mise).unwrap_or_else(|_| mise.clone());
        let version = CACHE.try_compute(key, || query_mise_version(&mise)).await?;

        Ok(Self { mise, version })
    }

    pub(crate) fn mise(&self) -> &Path {
        &self.mise
    }

    pub(crate) fn version(&self) -> &Version {
        &self.version
    }
}

async fn query_mise_version(mise: &Path) -> Result<Version> {
    // Even `mise --version` discovers config files and runs migrations and cache pruning.
    // Disposable directories isolate those side effects. CI disables the update check.
    let isolated = tempfile::tempdir()?;
    let root = isolated.path();

    let mut command = Cmd::new(mise);
    for key in inherited_mise_vars() {
        command.env_remove(key);
    }

    let output = command
        .current_dir(root)
        .env(EnvVars::CI, "1")
        .env(EnvVars::MISE_DATA_DIR, root.join("data"))
        .env(EnvVars::MISE_CACHE_DIR, root.join("cache"))
        .env(EnvVars::MISE_CONFIG_DIR, root.join("config"))
        .env(EnvVars::MISE_STATE_DIR, root.join("state"))
        .env(EnvVars::MISE_SYSTEM_CONFIG_DIR, root.join("system-config"))
        .env(EnvVars::MISE_SYSTEM_DATA_DIR, root.join("system-data"))
        .env(EnvVars::MISE_CEILING_PATHS, mise_ceiling(root)?)
        .env(EnvVars::MISE_NO_CONFIG, "1")
        .arg("--version")
        .check(true)
        .output()
        .await?;

    let stdout = str::from_utf8(&output.stdout)?;
    let version = stdout
        .split_whitespace()
        .next()
        .context("Failed to parse mise version output")?;
    version.parse().context("Failed to parse mise version")
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

        // mise pushes release tags before the release assets are published, so the newest
        // matching tag may not be downloadable yet. Only the newest few are worth probing.
        for version in self.list_versions(request).await?.into_iter().take(3) {
            trace!(%version, "Downloading mise");
            if let Some(result) = self.download(store, &version).await? {
                return Ok(result);
            }
            debug!(%version, "mise release is not published yet");
        }
        anyhow::bail!("No released mise version matches the request")
    }

    async fn find_installed(&self, request: &SemverRequest) -> Option<MiseResult> {
        let installed = installed_version_dirs::<Version>(&self.root)
            .sorted_unstable_by(|(a, _), (b, _)| b.cmp(a));

        for (version, path) in installed {
            let binary = mise_binary(&path);
            if !is_compatible(request, &version) || !is_executable(&binary) {
                continue;
            }

            match MiseResult::from_executable(binary).await {
                Ok(result) if result.version() == &version => return Some(result),
                Ok(result) => {
                    warn!(
                        expected = %version,
                        found = %result.version(),
                        path = %path.display(),
                        "Managed mise version mismatch"
                    );
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

    /// Lists the tagged mise versions matching `request`, newest first.
    async fn list_versions(&self, request: &SemverRequest) -> Result<Vec<Version>> {
        let output = git::git_cmd()?
            .arg("ls-remote")
            .arg("--tags")
            .arg("https://github.com/jdx/mise")
            .output()
            .await?;

        let stdout = str::from_utf8(&output.stdout)?;
        Ok(stdout
            .lines()
            .filter_map(|line| {
                let (_, reference) = line.split_once('\t')?;
                let tag = reference.strip_prefix("refs/tags/v")?;
                tag.parse().ok()
            })
            .filter(|version| is_compatible(request, version))
            .sorted_unstable_by(|a, b| b.cmp(a))
            .collect())
    }

    /// Downloads mise `version`, or returns `None` if its release is not published.
    async fn download(&self, store: &Store, version: &Version) -> Result<Option<MiseResult>> {
        let (platform, extension) = release_platform(&HOST)?;
        let filename = format!("mise-v{version}-{platform}.{extension}");
        let base_url = format!("https://github.com/jdx/mise/releases/download/v{version}");
        let url = format!("{base_url}/{filename}");
        let checksum_url = format!("{base_url}/SHASUMS256.txt");

        let download = match download_artifact(&url, &filename, store, async || {
            let Some(checksums) = fetch_checksum(&checksum_url).await? else {
                return Ok(None);
            };
            digest_from_sha256sums(&checksums, &filename)
        })
        .await
        {
            Ok(download) => download,
            // A tag without a published release has no assets, so its download 404s.
            Err(err) if is_not_found(&err) => return Ok(None),
            Err(err) => return Err(err.context("Failed to download mise")),
        };

        let install_dir = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&self.root)?;
        install_binary(download.path(), install_dir.path()).await?;

        let target = self.root.join(version.to_string());
        if target.exists() {
            fs_err::tokio::remove_dir_all(&target).await?;
        }
        crate::fs::rename_with_retry(install_dir.keep(), &target).await?;

        Ok(Some(MiseResult {
            mise: mise_binary(&target),
            version: version.clone(),
        }))
    }
}

async fn install_binary(download: &Path, install_dir: &Path) -> Result<()> {
    fs_err::tokio::create_dir_all(install_dir.join("bin")).await?;
    let binary = mise_binary(install_dir);

    if HOST.operating_system == OperatingSystem::Windows {
        fs_err::tokio::copy(download, &binary).await?;
    } else {
        let extracted = archive::extract_archive(download)
            .await
            .context("Failed to extract mise")?;
        crate::fs::rename_with_retry(mise_binary(&extracted), &binary).await?;
    }

    crate::fs::make_executable(&binary)?;
    Ok(())
}

fn mise_binary(prefix: &Path) -> PathBuf {
    prefix
        .join("bin")
        .join("mise")
        .with_extension(EXE_EXTENSION)
}

fn is_not_found(err: &anyhow::Error) -> bool {
    err.chain().any(|cause| {
        cause
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|err| err.status() == Some(reqwest::StatusCode::NOT_FOUND))
    })
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

    let is_musl = matches!(
        host.environment,
        Environment::Musl
            | Environment::Musleabi
            | Environment::Musleabihf
            | Environment::Muslabi64
    );
    let platform = if host.operating_system == OperatingSystem::Linux && is_musl {
        format!("{platform}-musl")
    } else {
        platform.to_string()
    };

    Ok((platform, extension))
}
