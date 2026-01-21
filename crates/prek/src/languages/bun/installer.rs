use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;
use target_lexicon::{Architecture, HOST, OperatingSystem};
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::git;
use crate::languages::bun::BunRequest;
use crate::languages::bun::version::BunVersion;
use crate::languages::download_and_extract;
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug)]
pub(crate) struct BunResult {
    bun: PathBuf,
    version: BunVersion,
}

impl Display for BunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.bun.display(), self.version)?;
        Ok(())
    }
}

/// Override the Bun binary name for testing.
static BUN_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars::var(EnvVars::PREK_INTERNAL__BUN_BINARY_NAME) {
        name
    } else {
        "bun".to_string()
    }
});

impl BunResult {
    pub(crate) fn from_executable(bun: PathBuf) -> Self {
        Self {
            bun,
            version: BunVersion::default(),
        }
    }

    pub(crate) fn from_dir(dir: &Path) -> Self {
        let bun = bin_dir(dir).join("bun").with_extension(EXE_EXTENSION);
        Self::from_executable(bun)
    }

    pub(crate) fn with_version(mut self, version: BunVersion) -> Self {
        self.version = version;
        self
    }

    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        let output = Cmd::new(&self.bun, "bun --version")
            .arg("--version")
            .check(true)
            .output()
            .await?;
        let output_str = String::from_utf8_lossy(&output.stdout);
        let version: BunVersion = output_str
            .trim()
            .parse()
            .context("Failed to parse bun version")?;

        self.version = version;

        Ok(self)
    }

    pub(crate) fn bun(&self) -> &Path {
        &self.bun
    }

    pub(crate) fn version(&self) -> &BunVersion {
        &self.version
    }
}

pub(crate) struct BunInstaller {
    root: PathBuf,
}

impl BunInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Install a version of Bun.
    pub(crate) async fn install(
        &self,
        store: &Store,
        request: &BunRequest,
        allows_download: bool,
    ) -> Result<BunResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;

        let _lock = LockedFile::acquire(self.root.join(".lock"), "bun").await?;

        if let Ok(bun_result) = self.find_installed(request) {
            trace!(%bun_result, "Found installed bun");
            return Ok(bun_result);
        }

        // Find all bun executables in PATH and check their versions
        if let Some(bun_result) = self.find_system_bun(request).await? {
            trace!(%bun_result, "Using system bun");
            return Ok(bun_result);
        }

        if !allows_download {
            anyhow::bail!("No suitable system Bun version found and downloads are disabled");
        }

        let resolved_version = self.resolve_version(request).await?;
        trace!(version = %resolved_version, "Downloading bun");

        self.download(store, &resolved_version).await
    }

    /// Get the installed version of Bun.
    fn find_installed(&self, req: &BunRequest) -> Result<BunResult> {
        let mut installed = fs_err::read_dir(&self.root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(err) => {
                    warn!(?err, "Failed to read entry");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_ok_and(|f| f.is_dir()))
            .filter_map(|entry| {
                let dir_name = entry.file_name();
                let version = BunVersion::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| a.cmp(b))
            .rev();

        installed
            .find_map(|(v, path)| {
                if req.matches(&v, Some(&path)) {
                    Some(BunResult::from_dir(&path).with_version(v))
                } else {
                    None
                }
            })
            .context("No installed bun version matches the request")
    }

    async fn resolve_version(&self, req: &BunRequest) -> Result<BunVersion> {
        // Latest versions come first, so we can find the latest matching version.
        let versions = self
            .list_remote_versions()
            .await
            .context("Failed to list remote versions")?;
        let version = versions
            .into_iter()
            .find(|version| req.matches(version, None))
            .context("Version not found on remote")?;
        Ok(version)
    }

    /// List all versions of Bun available on GitHub releases.
    async fn list_remote_versions(&self) -> Result<Vec<BunVersion>> {
        let output = git::git_cmd("list bun tags")?
            .arg("ls-remote")
            .arg("--tags")
            .arg("https://github.com/oven-sh/bun")
            .output()
            .await?
            .stdout;
        let output_str = str::from_utf8(&output)?;

        let versions: Vec<BunVersion> = output_str
            .lines()
            .filter_map(|line| {
                let reference = line.split('\t').nth(1)?;
                if reference.ends_with("^{}") {
                    return None;
                }

                let tag = reference.strip_prefix("refs/tags/")?;
                // Tags are in format "bun-v1.1.0".
                let tag = tag.strip_prefix("bun-v")?;
                BunVersion::from_str(tag).ok()
            })
            .sorted_unstable_by(|a, b| b.cmp(a))
            .collect();

        Ok(versions)
    }

    /// Install a specific version of Bun.
    async fn download(&self, store: &Store, version: &BunVersion) -> Result<BunResult> {
        let arch = match HOST.architecture {
            Architecture::X86_64 => "x64",
            Architecture::Aarch64(_) => "aarch64",
            _ => return Err(anyhow::anyhow!("Unsupported architecture")),
        };
        let os = match HOST.operating_system {
            OperatingSystem::Darwin(_) => "darwin",
            OperatingSystem::Linux => "linux",
            OperatingSystem::Windows => "windows",
            _ => return Err(anyhow::anyhow!("Unsupported OS")),
        };

        let filename = format!("bun-{os}-{arch}.zip");
        let url =
            format!("https://github.com/oven-sh/bun/releases/download/bun-v{version}/{filename}");
        let target = self.root.join(version.to_string());

        download_and_extract(&url, &filename, store, async |extracted| {
            if target.exists() {
                debug!(target = %target.display(), "Removing existing bun");
                fs_err::tokio::remove_dir_all(&target).await?;
            }

            // The ZIP extracts to bun-{os}-{arch}/bun, we need to move the contents
            // to {version}/bin/bun
            let extracted_binary = extracted.join("bun").with_extension(EXE_EXTENSION);
            let target_bin_dir = bin_dir(&target);
            fs_err::tokio::create_dir_all(&target_bin_dir).await?;

            let target_binary = target_bin_dir.join("bun").with_extension(EXE_EXTENSION);
            debug!(?extracted_binary, target = %target_binary.display(), "Moving bun to target");
            fs_err::tokio::rename(&extracted_binary, &target_binary).await?;

            anyhow::Ok(())
        })
        .await
        .context("Failed to download and extract bun")?;

        Ok(BunResult::from_dir(&target).with_version(version.clone()))
    }

    /// Find a suitable system Bun installation that matches the request.
    async fn find_system_bun(&self, bun_request: &BunRequest) -> Result<Option<BunResult>> {
        let bun_paths = match which::which_all(&*BUN_BINARY_NAME) {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No bun executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        // Check each bun executable for a matching version, stop early if found
        for bun_path in bun_paths {
            match BunResult::from_executable(bun_path).fill_version().await {
                Ok(bun_result) => {
                    // Check if this version matches the request
                    if bun_request.matches(&bun_result.version, Some(&bun_result.bun)) {
                        trace!(
                            %bun_result,
                            "Found a matching system bun"
                        );
                        return Ok(Some(bun_result));
                    }
                    trace!(
                        %bun_result,
                        "System bun does not match requested version"
                    );
                }
                Err(e) => {
                    warn!(?e, "Failed to get version for system bun");
                }
            }
        }

        debug!(?bun_request, "No system bun matches the requested version");
        Ok(None)
    }
}

pub(crate) fn bin_dir(prefix: &Path) -> PathBuf {
    // Bun installs global packages to $BUN_INSTALL/bin/ on all platforms
    prefix.join("bin")
}

pub(crate) fn lib_dir(prefix: &Path) -> PathBuf {
    if cfg!(windows) {
        prefix.join("node_modules")
    } else {
        prefix.join("lib").join("node_modules")
    }
}
