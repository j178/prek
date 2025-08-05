use crate::fs::LockedFile;
use crate::languages::golang::GoRequest;
use crate::languages::golang::version::GoVersion;
use crate::languages::node::NodeRequest;
use crate::process::Cmd;
use anyhow::{Context, Result};
use itertools::Itertools;
use reqwest::Client;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use target_lexicon::{Architecture, OperatingSystem, X86_32Architecture, HOST};
use tracing::{debug, trace, warn};

pub(crate) struct GoResult {
    path: PathBuf,
    version: GoVersion,
}

impl GoResult {
    fn new(path: PathBuf, version: GoVersion) -> Self {
        Self { path, version }
    }

    fn from_executable(path: PathBuf) -> Self {
        Self {
            path,
            version: GoVersion::default(),
        }
    }

    pub(crate) fn bin(&self) -> &Path {
        &self.path
    }

    pub(crate) fn version(&self) -> &GoVersion {
        &self.version
    }

    pub(crate) fn cmd(&self, summary: &str) -> Cmd {
        Cmd::new(&self.path, summary)
    }

    pub(crate) fn with_version(mut self, version: GoVersion) -> Self {
        self.version = version;
        self
    }

    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        let output = self.cmd("version").check(true).output().await?;
        // e.g. "go version go1.24.5 darwin/arm64"
        let version_str = String::from_utf8(output.stdout)?;
        let version_str = version_str.split_ascii_whitespace().nth(2).ok_or_else(|| {
            anyhow::anyhow!("Failed to parse Go version from output: {}", version_str)
        })?;

        let version = GoVersion::from_str(&version_str)?;

        self.version = version;

        Ok(self)
    }
}

pub(crate) struct GoInstaller {
    root: PathBuf,
    client: Client,
}

impl GoInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: Client::new(),
        }
    }

    pub(crate) async fn install(&self, request: &GoRequest) -> Result<GoResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;

        let _lock = LockedFile::acquire(self.root.join(".lock"), "go").await?;

        if let Ok(go) = self.find_installed(request) {
            trace!(%go, "Found installed go");
            return Ok(go);
        }

        if let Some(go) = self.find_system_go(request).await? {
            trace!(%go, "Using system go");
            return Ok(go);
        }

        let resolved_version = self
            .resolve_version(request)
            .await
            .context("Failed to resolve Go version")?;
        trace!(version = %resolved_version, "Installing go");

        self.download(&resolved_version).await
    }

    fn find_installed(&self, request: &GoRequest) -> Result<GoResult> {
        let mut installed = fs_err::read_dir(&self.root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(e) => {
                    warn!(?e, "Failed to read entry");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_ok_and(|f| f.is_dir()))
            .filter_map(|entry| {
                let dir_name = entry.file_name();
                let version = GoVersion::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| a.cmp(b))
            .rev();

        installed
            .find_map(|(version, path)| {
                if request.matches(&version) {
                    trace!(%version, "Found matching installed go");
                    Some(GoResult::new(path, version))
                } else {
                    trace!(%version, "Installed go does not match request");
                    None
                }
            })
            .context("No installed go version matches the request")
    }

    async fn resolve_version(&self, req: &GoRequest) -> Result<GoVersion> {
        let url = "https://go.dev/dl/?mode=json";
        let response = self.client.get(url).send().await?;
        let versions: Vec<GoVersion> = response.json().await?;

        let version = versions
            .into_iter()
            .find(|version| req.matches(version))
            .context("Version not found on remote")?;
        Ok(version)
    }

    async fn download(&self, version: &GoVersion) -> Result<GoResult> {
        let arch = match HOST.architecture {
            Architecture::X86_32(X86_32Architecture::I686) => "x86",
            Architecture::X86_64 => "x64",
            Architecture::Aarch64(_) => "arm64",
            Architecture::Arm(_) => "armv7l",
            Architecture::S390x => "s390x",
            Architecture::Powerpc => "ppc64",
            Architecture::Powerpc64le => "ppc64le",
            _ => return Err(anyhow::anyhow!("Unsupported architecture")),
        };
        let os = match HOST.operating_system {
            OperatingSystem::Darwin(_) => "darwin",
            OperatingSystem::Linux => "linux",
            OperatingSystem::Windows => "win",
            OperatingSystem::Aix => "aix",
            _ => return Err(anyhow::anyhow!("Unsupported OS")),
        };

        let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
        let filename = format!("go{version}.{os}-{arch}.tar.gz");
        let url = format!("https://go.dev/dl/go{}.tar.gz", version);

        let response = self.client.get(&url).send().await?;
        if !response.status().is_success() {
            return Err(anyhow::anyhow!(
                "Failed to download Go version {}: {}",
                version,
                response.status()
            ));
        }

        let tarball = response.bytes().await?;
        let tarball_path = self.root.join(format!("go{}.tar.gz", version));
        fs_err::tokio::write(&tarball_path, &tarball).await?;

        // Extract the tarball
        let tar_gz = fs_err::File::open(&tarball_path).await?;
        let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(tar_gz));
        archive.unpack(&self.root).await?;

        // Clean up the tarball
        fs_err::tokio::remove_file(tarball_path).await?;

        let go_path = self.root.join(format!("go{}", version));
        Ok(GoResult::new(go_path, version.clone()))
    }

    async fn find_system_node(&self, go_request: &GoRequest) -> Result<Option<GoResult>> {
        let go_paths: Vec<_> = match which::which_all("go") {
            Ok(paths) => paths.collect(),
            Err(e) => {
                debug!("No go executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        trace!(go_count = go_paths.len(), "Found go executables in PATH");

        // Check each go executable for a matching version, stop early if found
        for go_path in go_paths {
            match GoResult::from_executable(go_path).fill_version().await {
                Ok(go) => {
                    // Check if this version matches the request
                    if go_request.matches(go.version()) {
                        trace!(
                            %go,
                            "Found matching system go"
                        );
                        return Ok(Some(go_request));
                    }
                    trace!(
                        %go_request,
                        "System go does not match requested version"
                    );
                }
                Err(e) => {
                    warn!(?e, "Failed to get version for system go");
                }
            }
        }

        debug!("No system go matches the requested version");
        Ok(None)
    }
}
