use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::{Context, Result};
use itertools::Itertools;
use reqwest::Client;
use target_lexicon::{Architecture, HOST, OperatingSystem};
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::git;
use crate::languages::download_and_extract;
use crate::languages::golang::GoRequest;
use crate::languages::golang::golang::bin_dir;
use crate::languages::golang::version::GoVersion;
use crate::process::Cmd;

pub(crate) struct GoResult {
    path: PathBuf,
    version: GoVersion,
}

impl Display for GoResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.path.display(), self.version)?;
        Ok(())
    }
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

    pub(crate) fn from_dir(dir: &Path) -> Self {
        let go = bin_dir(dir).join("go").with_extension(EXE_EXTENSION);
        Self::from_executable(go)
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
        let output = self
            .cmd("go version")
            .arg("version")
            .check(true)
            .output()
            .await?;
        // e.g. "go version go1.24.5 darwin/arm64"
        let version_str = String::from_utf8(output.stdout)?;
        let version_str = version_str.split_ascii_whitespace().nth(2).ok_or_else(|| {
            anyhow::anyhow!("Failed to parse Go version from output: {}", version_str)
        })?;

        let version = GoVersion::from_str(version_str)?;

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
                if request.matches(&version, Some(&path)) {
                    trace!(%version, "Found matching installed go");
                    Some(GoResult::from_dir(&path).with_version(version))
                } else {
                    trace!(%version, "Installed go does not match request");
                    None
                }
            })
            .context("No installed go version matches the request")
    }

    async fn resolve_version(&self, req: &GoRequest) -> Result<GoVersion> {
        let output = git::git_cmd("list go tags")?
            .arg("ls-remote")
            .arg("--tags")
            .arg("https://github.com/golang/go")
            .output()
            .await?
            .stdout;
        let output_str = String::from_utf8(output)?;
        let versions: Vec<GoVersion> = output_str
            .lines()
            .filter_map(|line| {
                let tag = line.split('\t').nth(1)?;
                let tag = tag.strip_prefix("refs/tags/go")?;
                GoVersion::from_str(tag).ok()
            })
            .sorted_unstable_by(|a, b| b.cmp(a))
            .collect();

        let version = versions
            .into_iter()
            .find(|version| req.matches(version, None))
            .context("Version not found on remote")?;
        Ok(version)
    }

    async fn download(&self, version: &GoVersion) -> Result<GoResult> {
        let arch = match HOST.architecture {
            Architecture::X86_32(_) => "386",
            Architecture::X86_64 => "amd64",
            Architecture::Aarch64(_) => "arm64",
            Architecture::S390x => "s390x",
            Architecture::Powerpc => "ppc64",
            Architecture::Powerpc64le => "ppc64le",
            _ => return Err(anyhow::anyhow!("Unsupported architecture")),
        };
        let os = match HOST.operating_system {
            OperatingSystem::Darwin(_) => "darwin",
            OperatingSystem::Linux => "linux",
            OperatingSystem::Windows => "windows",
            OperatingSystem::Aix => "aix",
            OperatingSystem::Netbsd => "netbsd",
            OperatingSystem::Openbsd => "openbsd",
            OperatingSystem::Solaris => "solaris",
            OperatingSystem::Dragonfly => "dragonfly",
            OperatingSystem::Illumos => "illumos",
            _ => return Err(anyhow::anyhow!("Unsupported OS")),
        };

        let ext = if cfg!(windows) { "zip" } else { "tar.gz" };
        let filename = format!("go{version}.{os}-{arch}.{ext}");
        let url = format!("https://go.dev/dl/{filename}");
        let target = self.root.join(version.to_string());

        download_and_extract(&self.client, &url, &target, &filename, &self.root)
            .await
            .context("Failed to download and extract Go")?;

        Ok(GoResult::from_dir(&target).with_version(version.clone()))
    }

    async fn find_system_go(&self, go_request: &GoRequest) -> Result<Option<GoResult>> {
        let go_paths = match which::which_all("go") {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No go executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        for go_path in go_paths {
            match GoResult::from_executable(go_path).fill_version().await {
                Ok(go) => {
                    // Check if this version matches the request
                    if go_request.matches(&go.version, Some(&go.path)) {
                        trace!(
                            %go,
                            "Found matching system go"
                        );
                        return Ok(Some(go));
                    }
                    trace!(
                        %go,
                        "System go does not match requested version"
                    );
                }
                Err(e) => {
                    warn!(?e, "Failed to get version for system go");
                }
            }
        }

        debug!(?go_request, "No system go matches the requested version");
        Ok(None)
    }
}
