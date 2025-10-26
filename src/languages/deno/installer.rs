use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use constants::env_vars::EnvVars;
use itertools::Itertools;
use reqwest::Client;
use serde::Deserialize;
use target_lexicon::{Architecture, HOST, OperatingSystem};
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::languages::deno::{DenoRequest, DenoVersion};
use crate::languages::download_and_extract;
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug)]
pub(crate) struct DenoResult {
    deno: PathBuf,
    version: DenoVersion,
}

impl Display for DenoResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}@{}", self.deno.display(), self.version)
    }
}

/// Override the Deno binary name for testing.
static DENO_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars::var(EnvVars::PREK_INTERNAL__DENO_BINARY_NAME) {
        name
    } else {
        "deno".to_string()
    }
});

impl DenoResult {
    pub(crate) fn from_executable(deno: PathBuf) -> Self {
        Self {
            deno,
            version: DenoVersion::default(),
        }
    }

    pub(crate) fn from_dir(dir: &Path) -> Self {
        let deno = bin_dir(dir).join("deno").with_extension(EXE_EXTENSION);
        Self::from_executable(deno)
    }

    pub(crate) fn with_version(mut self, version: DenoVersion) -> Self {
        self.version = version;
        self
    }

    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        let output = Cmd::new(&self.deno, "deno --version")
            .arg("--version")
            .check(true)
            .output()
            .await?;
        // deno 1.40.0 (release, x86_64-unknown-linux-gnu)
        let version_str = String::from_utf8_lossy(&output.stdout);
        let version = version_str
            .split_whitespace()
            .nth(1)
            .context("Failed to get Deno version")?
            .parse::<DenoVersion>()
            .context("Failed to parse Deno version")?;

        self.version = version;

        Ok(self)
    }

    pub(crate) fn deno(&self) -> &Path {
        &self.deno
    }

    pub(crate) fn version(&self) -> &DenoVersion {
        &self.version
    }
}

pub(crate) struct DenoInstaller {
    root: PathBuf,
    client: Client,
}

impl DenoInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: Client::new(),
        }
    }

    /// Install a version of Deno.
    pub(crate) async fn install(
        &self,
        store: &Store,
        request: &DenoRequest,
        allows_download: bool,
    ) -> Result<DenoResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;

        let _lock = LockedFile::acquire(self.root.join(".lock"), "deno").await?;

        if let Ok(deno_result) = self.find_installed(request) {
            trace!(%deno_result, "Found installed deno");
            return Ok(deno_result);
        }

        // Find all deno executables in PATH and check their versions
        if let Some(deno_result) = self.find_system_deno(request).await? {
            trace!(%deno_result, "Using system deno");
            return Ok(deno_result);
        }

        if !allows_download {
            anyhow::bail!("No suitable system Deno version found and downloads are disabled");
        }

        let resolved_version = self.resolve_version(request).await?;
        trace!(version = %resolved_version, "Downloading deno");

        self.download(store, &resolved_version).await
    }

    /// Get the installed version of Deno.
    fn find_installed(&self, req: &DenoRequest) -> Result<DenoResult> {
        fs_err::read_dir(&self.root)
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
            .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
            .filter_map(|entry| {
                let dir_name = entry.file_name();
                let version = DenoVersion::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(version_a, _), (version_b, _)| version_b.cmp(version_a))
            .find_map(|(version, path)| {
                req.matches(&version, Some(&path))
                    .then(|| DenoResult::from_dir(&path).with_version(version))
            })
            .context("No installed deno version matches the request")
    }

    async fn resolve_version(&self, req: &DenoRequest) -> Result<DenoVersion> {
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

    /// List all versions of Deno available on GitHub releases.
    async fn list_remote_versions(&self) -> Result<Vec<DenoVersion>> {
        #[derive(Deserialize)]
        struct Release {
            tag_name: String,
        }

        let url = "https://api.github.com/repos/denoland/deno/releases?per_page=100";
        let releases: Vec<Release> = self
            .client
            .get(url)
            .header("User-Agent", "prek")
            .send()
            .await?
            .json()
            .await?;

        let versions: Vec<DenoVersion> = releases
            .into_iter()
            .filter_map(|release| DenoVersion::from_str(&release.tag_name).ok())
            .sorted_unstable_by(|version_a, version_b| version_b.cmp(version_a))
            .collect();

        // Get more pages if needed (GitHub paginates at 100 items)
        if versions.is_empty() {
            anyhow::bail!("No Deno versions found on GitHub");
        }

        Ok(versions)
    }

    /// Install a specific version of Deno.
    async fn download(&self, store: &Store, version: &DenoVersion) -> Result<DenoResult> {
        let arch = match HOST.architecture {
            Architecture::X86_64 => "x86_64",
            Architecture::Aarch64(_) => "aarch64",
            _ => return Err(anyhow::anyhow!("Unsupported architecture for Deno")),
        };

        let os = match HOST.operating_system {
            OperatingSystem::Darwin(_) => "apple-darwin",
            OperatingSystem::Linux => "unknown-linux-gnu",
            OperatingSystem::Windows => "pc-windows-msvc",
            _ => return Err(anyhow::anyhow!("Unsupported OS for Deno")),
        };

        let ext = "zip"; // Deno uses zip for all platforms
        let filename = format!("deno-{arch}-{os}.{ext}");
        let url =
            format!("https://github.com/denoland/deno/releases/download/v{version}/{filename}");
        let target = self.root.join(version.to_string());

        download_and_extract(&self.client, &url, &filename, store, async |extracted| {
            // Deno comes as a single binary in the zip root.
            // After strip_component, `extracted` points to the deno binary itself (not a directory).
            // We need to move this file to our target bin directory.

            // Check if extracted is the binary file itself
            let deno_binary = if extracted.is_file() {
                extracted.to_path_buf()
            } else {
                // It's a directory, look for deno inside
                extracted.join("deno").with_extension(EXE_EXTENSION)
            };

            if !deno_binary.exists() {
                anyhow::bail!(
                    "Deno binary not found. Expected at {}",
                    deno_binary.display()
                );
            }

            if target.exists() {
                debug!(target = %target.display(), "Removing existing deno");
                fs_err::tokio::remove_dir_all(&target).await?;
            }

            // Create target directory and bin subdirectory
            let bin_dir_path = bin_dir(&target);
            fs_err::tokio::create_dir_all(&bin_dir_path).await?;

            // Move/copy deno binary to bin/
            let target_binary = bin_dir_path.join("deno").with_extension(EXE_EXTENSION);

            debug!(
                from = %deno_binary.display(),
                to = %target_binary.display(),
                "Moving deno binary"
            );

            // Use copy + remove instead of rename since source might be on different filesystem
            fs_err::tokio::copy(&deno_binary, &target_binary).await?;

            // Set executable permissions on Unix
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let mut perms = fs_err::tokio::metadata(&target_binary).await?.permissions();
                perms.set_mode(0o755);
                fs_err::tokio::set_permissions(&target_binary, perms).await?;
            }

            anyhow::Ok(())
        })
        .await
        .context("Failed to download and extract deno")?;

        Ok(DenoResult::from_dir(&target).with_version(version.clone()))
    }

    /// Find a suitable system Deno installation that matches the request.
    async fn find_system_deno(&self, deno_request: &DenoRequest) -> Result<Option<DenoResult>> {
        let deno_paths = match which::which_all(&*DENO_BINARY_NAME) {
            Ok(paths) => paths,
            Err(error) => {
                debug!("No deno executables found in PATH: {}", error);
                return Ok(None);
            }
        };

        // Check each deno executable for a matching version, stop early if found
        for deno_path in deno_paths {
            match DenoResult::from_executable(deno_path).fill_version().await {
                Ok(deno_result) => {
                    // Check if this version matches the request
                    if deno_request.matches(&deno_result.version, Some(&deno_result.deno)) {
                        trace!(
                            %deno_result,
                            "Found a matching system deno"
                        );
                        return Ok(Some(deno_result));
                    }
                    trace!(
                        %deno_result,
                        "System deno does not match requested version"
                    );
                }
                Err(error) => {
                    warn!(?error, "Failed to get version for system deno");
                }
            }
        }

        debug!(
            ?deno_request,
            "No system deno matches the requested version"
        );
        Ok(None)
    }
}

pub(crate) fn bin_dir(prefix: &Path) -> PathBuf {
    if cfg!(windows) {
        prefix.to_path_buf()
    } else {
        prefix.join("bin")
    }
}
