use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;
use tracing::{debug, trace, warn};

use super::version::DotnetVersion;
use crate::fs::LockedFile;
use crate::http::REQWEST_CLIENT;
use crate::languages::dotnet::DotnetRequest;
use crate::process::Cmd;

static DOTNET_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars::var(EnvVars::PREK_INTERNAL_DOTNET_BINARY_NAME) {
        name
    } else {
        "dotnet".to_string()
    }
});

/// Result of a dotnet installation or discovery.
#[derive(Debug, Clone)]
pub(crate) struct DotnetResult {
    dotnet: PathBuf,
    version: DotnetVersion,
}

impl Display for DotnetResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.dotnet.display(), self.version)?;
        Ok(())
    }
}

impl DotnetResult {
    /// Creates a result from a `dotnet` executable path without probing version yet.
    pub(crate) fn from_executable(dotnet: PathBuf) -> Self {
        Self {
            dotnet,
            version: DotnetVersion::default(),
        }
    }

    /// Creates a result from a managed installation directory.
    pub(crate) fn from_dir(dir: &Path) -> Self {
        let dotnet = dir.join("dotnet").with_extension(EXE_EXTENSION);
        Self::from_executable(dotnet)
    }

    /// Returns the resolved `dotnet` executable path.
    pub(crate) fn dotnet(&self) -> &Path {
        &self.dotnet
    }

    /// Returns the probed SDK version.
    pub(crate) fn version(&self) -> &DotnetVersion {
        &self.version
    }

    /// Replaces the stored SDK version.
    pub(crate) fn with_version(mut self, version: DotnetVersion) -> Self {
        self.version = version;
        self
    }

    /// Builds a command that runs this `dotnet` executable.
    pub(crate) fn cmd(&self, summary: &str) -> Cmd {
        Cmd::new(&self.dotnet, summary)
    }

    /// Fills the SDK version by running `dotnet --version`.
    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        let mut cmd = self.cmd("get dotnet version");
        if let Some(parent) = self.dotnet.parent() {
            cmd.current_dir(parent);
        }

        let stdout = cmd.arg("--version").check(true).output().await?.stdout;
        let version_str = str::from_utf8(&stdout)?.trim();
        let version = version_str
            .parse()
            .with_context(|| format!("Failed to parse version from: {version_str}"))?;

        self.version = version;

        Ok(self)
    }
}

/// Finds, downloads, and manages SDK installations for the `dotnet` backend.
pub(crate) struct DotnetInstaller {
    /// The base directory for all managed dotnet installations (e.g., .../tools/dotnet)
    root: PathBuf,
}

impl DotnetInstaller {
    /// Creates an installer rooted at the managed `dotnet` tool directory.
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Install or find dotnet SDK based on the language request.
    pub(crate) async fn install(
        &self,
        request: &DotnetRequest,
        allows_download: bool,
    ) -> Result<DotnetResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;
        let _lock = LockedFile::acquire(self.root.join(".lock"), "dotnet").await?;

        if let Ok(result) = self.find_installed(request) {
            debug!(%result, "Using existing managed dotnet");
            return Ok(result);
        }

        if let Some(result) = self.find_system_dotnet(request).await? {
            debug!(%result, "Using system dotnet");
            return Ok(result);
        }

        if !allows_download {
            bail!("No suitable dotnet version found and downloads are disabled");
        }

        self.install_managed(request).await
    }

    /// Finds the newest managed SDK installation that satisfies the request.
    fn find_installed(&self, request: &DotnetRequest) -> Result<DotnetResult> {
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
            .filter(|entry| entry.file_type().is_ok_and(|file_type| file_type.is_dir()))
            .filter_map(|entry| {
                let dir_name = entry.file_name();
                let version = DotnetVersion::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| a.cmp(b))
            .rev();

        installed
            .find_map(|(version, path)| {
                if request.matches(&version) {
                    Some(DotnetResult::from_dir(&path).with_version(version))
                } else {
                    None
                }
            })
            .context("No installed dotnet version matches the request")
    }

    /// Finds the first system `dotnet` executable in `PATH` that satisfies the request.
    async fn find_system_dotnet(&self, request: &DotnetRequest) -> Result<Option<DotnetResult>> {
        let dotnet_paths = match which::which_all(&*DOTNET_BINARY_NAME) {
            Ok(paths) => paths,
            Err(err) => {
                debug!("No dotnet executables found in PATH: {err}");
                return Ok(None);
            }
        };

        for dotnet in dotnet_paths {
            match DotnetResult::from_executable(dotnet).fill_version().await {
                Ok(result) => {
                    if request.matches(result.version()) {
                        trace!(%result, "Found system dotnet that matches request");
                        return Ok(Some(result));
                    }
                    trace!(%result, "System dotnet does not match request");
                }
                Err(e) => {
                    warn!(?e, "Failed to query version for system dotnet");
                }
            }
        }

        Ok(None)
    }

    /// Downloads an SDK into a temporary directory, then promotes it to its final location.
    async fn install_managed(&self, request: &DotnetRequest) -> Result<DotnetResult> {
        let install_dir = tempfile::Builder::new()
            .prefix(".install-")
            .tempdir_in(&self.root)?;

        debug!(
            request = ?request,
            path = %install_dir.path().display(),
            "Installing dotnet SDK"
        );

        self.download(install_dir.path(), request).await?;

        let installed = DotnetResult::from_dir(install_dir.path())
            .fill_version()
            .await
            .context("Failed to query installed dotnet version")?;

        self.promote_installation(install_dir, installed).await
    }

    /// Renames a successful temporary installation into its versioned final directory.
    async fn promote_installation(
        &self,
        install_dir: tempfile::TempDir,
        installed: DotnetResult,
    ) -> Result<DotnetResult> {
        let DotnetResult { version, .. } = installed;

        let final_dir = self.root.join(version.to_string());
        if final_dir.exists() {
            warn!(
                path = %final_dir.display(),
                "Final installation directory already exists, removing"
            );
            fs_err::tokio::remove_dir_all(&final_dir).await?;
        }

        let install_path = install_dir.keep();
        fs_err::tokio::rename(&install_path, &final_dir).await?;

        Ok(DotnetResult::from_dir(&final_dir).with_version(version))
    }

    /// Downloads the platform-specific install script and runs it for the request.
    async fn download(&self, install_dir: &Path, request: &DotnetRequest) -> Result<()> {
        // https://learn.microsoft.com/en-us/dotnet/core/tools/dotnet-install-script
        let (script_url, script_name) = if cfg!(windows) {
            (
                "https://dot.net/v1/dotnet-install.ps1",
                "dotnet-install.ps1",
            )
        } else {
            ("https://dot.net/v1/dotnet-install.sh", "dotnet-install.sh")
        };
        let script_dir = tempfile::tempdir()?;
        let script_path = script_dir.path().join(script_name);

        let response = REQWEST_CLIENT
            .get(script_url)
            .send()
            .await?
            .error_for_status()
            .with_context(|| {
                format!("Failed to download dotnet install script from `{script_url}`")
            })?;
        let script_content = response.bytes().await?;
        fs_err::tokio::write(&script_path, &script_content).await?;

        Self::install_dotnet(&script_path, install_dir, request).await
    }

    #[cfg(unix)]
    /// Executes `dotnet-install.sh` for Unix-like platforms.
    async fn install_dotnet(
        script_path: &Path,
        install_dir: &Path,
        request: &DotnetRequest,
    ) -> Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs_err::tokio::metadata(script_path).await?.permissions();
        perms.set_mode(0o755);
        fs_err::tokio::set_permissions(script_path, perms).await?;

        let mut cmd = Cmd::new(script_path, "dotnet-install.sh");
        cmd.arg("--no-path").arg("--install-dir").arg(install_dir);
        match request {
            DotnetRequest::Any => {
                cmd.arg("--channel").arg("LTS");
            }
            DotnetRequest::Channel(channel) => {
                cmd.arg("--channel").arg(channel.to_string());
            }
            DotnetRequest::Exact(major, minor, patch) => {
                cmd.arg("--version").arg(format!("{major}.{minor}.{patch}"));
            }
        }

        cmd.check(true).output().await?;
        Ok(())
    }

    #[cfg(windows)]
    /// Executes `dotnet-install.ps1` for Windows.
    async fn install_dotnet(
        script_path: &Path,
        install_dir: &Path,
        request: &DotnetRequest,
    ) -> Result<()> {
        let mut cmd = Cmd::new("powershell.exe", "dotnet-install.ps1");
        cmd.arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-NonInteractive")
            .arg("-File")
            .arg(script_path)
            .arg("-NoPath")
            .arg("-InstallDir")
            .arg(install_dir);
        match request {
            DotnetRequest::Any => {
                cmd.arg("-Channel").arg("LTS");
            }
            DotnetRequest::Channel(channel) => {
                cmd.arg("-Channel").arg(channel.to_string());
            }
            DotnetRequest::Exact(major, minor, patch) => {
                cmd.arg("-Version").arg(format!("{major}.{minor}.{patch}"));
            }
        }

        cmd.check(true).output().await?;
        Ok(())
    }
}
