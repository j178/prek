use std::fmt::Display;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use semver::Version;
use tracing::debug;

use crate::fs::LockedFile;
use crate::languages::dotnet::DotnetRequest;
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::store::{Store, ToolBucket};

/// Result of a dotnet installation or discovery.
#[derive(Debug)]
pub(crate) struct DotnetResult {
    dotnet: PathBuf,
    version: Version,
}

impl Display for DotnetResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.dotnet.display(), self.version)?;
        Ok(())
    }
}

impl DotnetResult {
    pub(crate) fn new(dotnet: PathBuf, version: Version) -> Self {
        Self { dotnet, version }
    }

    pub(crate) fn dotnet(&self) -> &Path {
        &self.dotnet
    }

    pub(crate) fn version(&self) -> &Version {
        &self.version
    }
}

pub(crate) struct DotnetInstaller {
    root: PathBuf,
}

impl DotnetInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Install or find dotnet SDK based on the language request.
    pub(crate) async fn install(
        &self,
        request: &LanguageRequest,
        allows_download: bool,
    ) -> Result<DotnetResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;
        let _lock = LockedFile::acquire(self.root.join(".lock"), "dotnet").await?;

        // First, try to find a system dotnet that satisfies the request
        if let Some(result) = self.find_system_dotnet(request).await? {
            debug!(%result, "Using system dotnet");
            return Ok(result);
        }

        // Check if we have a managed installation that satisfies the request
        if let Some(result) = self.find_installed(request).await? {
            debug!(%result, "Using managed dotnet");
            return Ok(result);
        }

        // If system_only is requested and we didn't find a matching system version, fail
        if matches!(request, LanguageRequest::Any { system_only: true }) {
            anyhow::bail!("No system dotnet installation found");
        }

        if !allows_download {
            anyhow::bail!("No suitable dotnet version found and downloads are disabled");
        }

        // Install dotnet SDK
        let version_request = to_dotnet_install_version(request);
        debug!("Installing dotnet SDK");
        self.download(version_request.as_deref()).await?;

        let dotnet_exe = dotnet_executable(&self.root);
        if !dotnet_exe.exists() {
            anyhow::bail!(
                "dotnet installation failed: executable not found at {}",
                dotnet_exe.display()
            );
        }

        let version = query_dotnet_version(&dotnet_exe).await?;
        Ok(DotnetResult::new(dotnet_exe, version))
    }

    async fn find_system_dotnet(&self, request: &LanguageRequest) -> Result<Option<DotnetResult>> {
        let Ok(system_dotnet) = which::which("dotnet") else {
            return Ok(None);
        };

        let Ok(version) = query_dotnet_version(&system_dotnet).await else {
            return Ok(None);
        };

        if version_satisfies_request(&version, request) {
            Ok(Some(DotnetResult::new(system_dotnet, version)))
        } else {
            Ok(None)
        }
    }

    async fn find_installed(&self, request: &LanguageRequest) -> Result<Option<DotnetResult>> {
        let dotnet_exe = dotnet_executable(&self.root);
        if !dotnet_exe.exists() {
            return Ok(None);
        }

        let Ok(version) = query_dotnet_version(&dotnet_exe).await else {
            return Ok(None);
        };

        if version_satisfies_request(&version, request) {
            Ok(Some(DotnetResult::new(dotnet_exe, version)))
        } else {
            Ok(None)
        }
    }

    /// Install dotnet SDK using the official install script.
    async fn download(&self, version: Option<&str>) -> Result<()> {
        #[cfg(unix)]
        {
            self.install_dotnet_unix(version).await
        }

        #[cfg(windows)]
        {
            self.install_dotnet_windows(version).await
        }
    }

    #[cfg(unix)]
    async fn install_dotnet_unix(&self, version: Option<&str>) -> Result<()> {
        // Download the install script
        let script_url = "https://dot.net/v1/dotnet-install.sh";
        let script_path = self.root.join("dotnet-install.sh");

        let response = reqwest::get(script_url)
            .await
            .context("Failed to download dotnet-install.sh")?;
        let script_content = response
            .bytes()
            .await
            .context("Failed to read dotnet-install.sh")?;
        fs_err::tokio::write(&script_path, &script_content).await?;

        // Make script executable
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs_err::tokio::metadata(&script_path).await?.permissions();
            perms.set_mode(0o755);
            fs_err::tokio::set_permissions(&script_path, perms).await?;
        }

        // Run the install script
        let mut cmd = Cmd::new("bash", "dotnet-install.sh");
        cmd.arg(&script_path).arg("--install-dir").arg(&self.root);

        if let Some(ver) = version {
            cmd.arg("--channel").arg(ver);
        } else {
            // Default to LTS
            cmd.arg("--channel").arg("LTS");
        }

        cmd.check(true)
            .output()
            .await
            .context("Failed to run dotnet-install.sh")?;

        Ok(())
    }

    #[cfg(windows)]
    async fn install_dotnet_windows(&self, version: Option<&str>) -> Result<()> {
        // Download the install script
        let script_url = "https://dot.net/v1/dotnet-install.ps1";
        let script_path = self.root.join("dotnet-install.ps1");

        let response = reqwest::get(script_url)
            .await
            .context("Failed to download dotnet-install.ps1")?;
        let script_content = response
            .bytes()
            .await
            .context("Failed to read dotnet-install.ps1")?;
        fs_err::tokio::write(&script_path, &script_content).await?;

        // Run the install script
        let mut cmd = Cmd::new("powershell", "dotnet-install.ps1");
        cmd.arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-File")
            .arg(&script_path)
            .arg("-InstallDir")
            .arg(&self.root);

        if let Some(ver) = version {
            cmd.arg("-Channel").arg(ver);
        } else {
            // Default to LTS
            cmd.arg("-Channel").arg("LTS");
        }

        cmd.check(true)
            .output()
            .await
            .context("Failed to run dotnet-install.ps1")?;

        Ok(())
    }
}

/// Query the version of a dotnet executable.
pub(crate) async fn query_dotnet_version(dotnet: &Path) -> Result<Version> {
    let stdout = Cmd::new(dotnet, "get dotnet version")
        .arg("--version")
        .check(true)
        .output()
        .await?
        .stdout;

    let version_str = String::from_utf8_lossy(&stdout).trim().to_string();
    parse_dotnet_version(&version_str).context("Failed to parse dotnet version")
}

/// Parse dotnet version string to semver.
/// .NET versions can be like "8.0.100", "9.0.100-preview.1.24101.2", etc.
pub(crate) fn parse_dotnet_version(version_str: &str) -> Option<Version> {
    // Strip any pre-release suffix for parsing
    let base_version = version_str.split('-').next()?;
    let parts: Vec<&str> = base_version.split('.').collect();
    if parts.len() >= 2 {
        let major: u64 = parts[0].parse().ok()?;
        let minor: u64 = parts[1].parse().ok()?;
        let patch: u64 = parts.get(2).and_then(|p| p.parse().ok()).unwrap_or(0);
        Some(Version::new(major, minor, patch))
    } else {
        None
    }
}

pub(crate) fn dotnet_executable(dotnet_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dotnet_dir.join("dotnet.exe")
    } else {
        dotnet_dir.join("dotnet")
    }
}

fn version_satisfies_request(version: &Version, request: &LanguageRequest) -> bool {
    match request {
        LanguageRequest::Any { .. } => true,
        LanguageRequest::Dotnet(req) => match req {
            DotnetRequest::Any => true,
            DotnetRequest::Major(major) => version.major == *major,
            DotnetRequest::MajorMinor(major, minor) => {
                version.major == *major && version.minor == *minor
            }
            DotnetRequest::MajorMinorPatch(major, minor, patch) => {
                version.major == *major && version.minor == *minor && version.patch == *patch
            }
        },
        _ => true,
    }
}

fn to_dotnet_install_version(request: &LanguageRequest) -> Option<String> {
    match request {
        LanguageRequest::Any { .. } => None,
        LanguageRequest::Dotnet(req) => req.to_install_version(),
        _ => None,
    }
}

/// Create a `DotnetInstaller` from the store.
pub(crate) fn installer_from_store(store: &Store) -> DotnetInstaller {
    let dotnet_dir = store.tools_path(ToolBucket::Dotnet);
    DotnetInstaller::new(dotnet_dir)
}

#[cfg(test)]
mod tests {
    use super::parse_dotnet_version;

    #[test]
    fn test_parse_stable_version() {
        let version = parse_dotnet_version("8.0.100").unwrap();
        assert_eq!(version.major, 8);
        assert_eq!(version.minor, 0);
        assert_eq!(version.patch, 100);
    }

    #[test]
    fn test_parse_preview_version() {
        let version = parse_dotnet_version("9.0.100-preview.1.24101.2").unwrap();
        assert_eq!(version.major, 9);
        assert_eq!(version.minor, 0);
        assert_eq!(version.patch, 100);
    }

    #[test]
    fn test_parse_rc_version() {
        let version = parse_dotnet_version("8.0.0-rc.1.23419.4").unwrap();
        assert_eq!(version.major, 8);
        assert_eq!(version.minor, 0);
        assert_eq!(version.patch, 0);
    }

    #[test]
    fn test_parse_two_part_version() {
        let version = parse_dotnet_version("8.0").unwrap();
        assert_eq!(version.major, 8);
        assert_eq!(version.minor, 0);
        assert_eq!(version.patch, 0);
    }

    #[test]
    fn test_parse_invalid_version() {
        assert!(parse_dotnet_version("").is_none());
        assert!(parse_dotnet_version("invalid").is_none());
    }
}
