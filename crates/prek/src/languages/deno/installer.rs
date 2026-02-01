use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;
use serde::Deserialize;
use target_lexicon::{Architecture, HOST, OperatingSystem};
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::languages::deno::DenoRequest;
use crate::languages::deno::version::DenoVersion;
use crate::languages::{REQWEST_CLIENT, download_and_extract};
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug)]
pub(crate) struct DenoResult {
    deno: PathBuf,
    version: DenoVersion,
}

impl Display for DenoResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.deno.display(), self.version)?;
        Ok(())
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
        // Output format: "deno 2.1.0 (release, x86_64-unknown-linux-gnu)\n..."
        let output_str = String::from_utf8_lossy(&output.stdout);
        let version_str = output_str
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("deno "))
            .and_then(|rest| rest.split_whitespace().next())
            .context("Failed to parse deno version output")?;

        self.version = version_str
            .parse()
            .context("Failed to parse deno version")?;

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
}

impl DenoInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
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
                let version = DenoVersion::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| a.cmp(b))
            .rev();

        installed
            .find_map(|(v, path)| {
                if req.matches(&v, Some(&path)) {
                    Some(DenoResult::from_dir(&path).with_version(v))
                } else {
                    None
                }
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

    /// List all versions of Deno available from the official versions endpoint.
    ///
    /// Uses <https://deno.com/versions.json> which is lightweight and doesn't
    /// have rate-limit issues like the GitHub API.
    async fn list_remote_versions(&self) -> Result<Vec<DenoVersion>> {
        #[derive(Deserialize)]
        struct VersionsResponse {
            cli: Vec<String>,
        }

        let url = "https://deno.com/versions.json";
        let response: VersionsResponse = REQWEST_CLIENT.get(url).send().await?.json().await?;

        // Versions are already sorted in descending order (newest first)
        let versions: Vec<DenoVersion> = response
            .cli
            .into_iter()
            .filter_map(|v| DenoVersion::from_str(&v).ok())
            .collect();

        if versions.is_empty() {
            anyhow::bail!("No Deno versions found");
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

        let filename = format!("deno-{arch}-{os}.zip");
        let url = format!("https://dl.deno.land/release/v{version}/{filename}");
        let target = self.root.join(version.to_string());

        download_and_extract(&url, &filename, store, async |extracted| {
            if target.exists() {
                debug!(target = %target.display(), "Removing existing deno");
                fs_err::tokio::remove_dir_all(&target).await?;
            }

            // Deno ZIP contains just the binary at the root level.
            // After strip_component, `extracted` may be the binary itself (if singular)
            // or a directory containing the binary.
            let extracted_binary = if extracted.is_file() {
                extracted.to_path_buf()
            } else {
                extracted.join("deno").with_extension(EXE_EXTENSION)
            };

            let target_bin_dir = bin_dir(&target);
            fs_err::tokio::create_dir_all(&target_bin_dir).await?;

            let target_binary = target_bin_dir.join("deno").with_extension(EXE_EXTENSION);
            debug!(?extracted_binary, target = %target_binary.display(), "Moving deno to target");
            fs_err::tokio::rename(&extracted_binary, &target_binary).await?;

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
            Err(e) => {
                debug!("No deno executables found in PATH: {}", e);
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
                Err(e) => {
                    warn!(?e, "Failed to get version for system deno");
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
    prefix.join("bin")
}
