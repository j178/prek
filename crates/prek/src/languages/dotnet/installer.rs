use std::fmt::Display;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use semver::Version;
use tracing::debug;

use crate::fs::LockedFile;
use crate::http::REQWEST_CLIENT;
use crate::languages::dotnet::DotnetRequest;
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::store::{Store, ToolBucket};

/// Result of a dotnet installation or discovery.
#[derive(Debug, Clone)]
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
    /// The base directory for all managed dotnet installations (e.g., .../tools/dotnet)
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

        if let Some(result) = self.find_system_dotnet(request).await? {
            debug!(%result, "Using system dotnet");
            return Ok(result);
        }

        if let Some(result) = self.find_installed(request).await? {
            debug!(%result, "Using existing managed dotnet");
            return Ok(result);
        }

        if matches!(request, LanguageRequest::Any { system_only: true }) {
            bail!("No system dotnet installation found");
        }

        if !allows_download {
            bail!("No suitable dotnet version found and downloads are disabled");
        }

        // We use the requested version string to determine the target directory.
        // If no version is specified (e.g. "LTS"), the install script will resolve it.
        let version_str = to_dotnet_install_version(request);
        let target_dir_name = version_str.as_deref().unwrap_or("default");
        let install_dir = self.root.join(target_dir_name);

        // If the directory already exists but find_installed missed it, it might be partial.
        // We clean it to ensure a fresh, valid install.
        if install_dir.exists() {
            fs_err::tokio::remove_dir_all(&install_dir).await?;
        }
        fs_err::tokio::create_dir_all(&install_dir).await?;

        debug!(request = ?version_str, path = %install_dir.display(), "Installing dotnet SDK");
        self.download(&install_dir, version_str.as_deref()).await?;

        // Verify the installation and get the actual specific version (e.g. 8.0.401)
        let installed = self
            .query_installation_at(&install_dir)
            .await
            .context("Failed to verify newly installed dotnet")?;

        let final_dir = self.root.join(installed.version().to_string());
        if install_dir != final_dir {
            if final_dir.exists() {
                // Verify the existing final_dir is healthy before using it
                if let Ok(existing_result) = self.query_installation_at(&final_dir).await {
                    // Existing installation is healthy, remove our new one and use existing
                    fs_err::tokio::remove_dir_all(&install_dir).await?;
                    return Ok(existing_result);
                }
                // Existing installation is corrupt, replace it with our new one
                debug!(
                    "Existing installation at {} is corrupt, replacing with new installation",
                    final_dir.display()
                );
                fs_err::tokio::remove_dir_all(&final_dir).await?;
                fs_err::tokio::rename(&install_dir, &final_dir).await?;
            } else {
                fs_err::tokio::rename(&install_dir, &final_dir).await?;
            }
            return Ok(DotnetResult::new(
                dotnet_executable(&final_dir),
                installed.version().clone(),
            ));
        }

        Ok(installed)
    }

    /// Scans the root directory for all subdirectories and finds the first one matching the request.
    async fn find_installed(&self, request: &LanguageRequest) -> Result<Option<DotnetResult>> {
        if !self.root.exists() {
            return Ok(None);
        }
        let mut entries = fs_err::tokio::read_dir(&self.root).await?;
        let mut found_versions = Vec::new();

        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if !path.is_dir()
                || path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|s| s.starts_with('.'))
            {
                continue;
            }

            // Check if this directory contains a valid dotnet installation
            if let Ok(version) = query_dotnet_version(&dotnet_executable(&path)).await {
                if version_satisfies_request(&version, request) {
                    found_versions.push(DotnetResult::new(dotnet_executable(&path), version));
                }
            }
        }

        // Sort by version descending to pick the newest compatible version
        found_versions.sort_by(|a, b| b.version().cmp(a.version()));
        Ok(found_versions.into_iter().next())
    }

    async fn find_system_dotnet(&self, request: &LanguageRequest) -> Result<Option<DotnetResult>> {
        if let Ok(system_dotnet) = which::which("dotnet") {
            if let Ok(version) = query_dotnet_version(&system_dotnet).await {
                if version_satisfies_request(&version, request) {
                    return Ok(Some(DotnetResult::new(system_dotnet, version)));
                }
            }
        }
        Ok(None)
    }

    async fn query_installation_at(&self, install_dir: &Path) -> Result<DotnetResult> {
        let dotnet_exe = dotnet_executable(install_dir);
        if !dotnet_exe.exists() {
            bail!("dotnet executable not found at {}", dotnet_exe.display());
        }
        let version = query_dotnet_version(&dotnet_exe).await?;
        Ok(DotnetResult::new(dotnet_exe, version))
    }

    async fn download(&self, install_dir: &Path, version: Option<&str>) -> Result<()> {
        #[cfg(unix)]
        {
            self.install_dotnet_unix(install_dir, version).await
        }

        #[cfg(windows)]
        {
            self.install_dotnet_windows(install_dir, version).await
        }
    }

    #[cfg(unix)]
    async fn install_dotnet_unix(&self, install_dir: &Path, version: Option<&str>) -> Result<()> {
        let script_url = "https://dot.net/v1/dotnet-install.sh";
        let script_path = install_dir.join("dotnet-install.sh");

        let response = REQWEST_CLIENT
            .get(script_url)
            .send()
            .await?
            .error_for_status()
            .with_context(|| {
                format!("failed to download dotnet install script from {script_url}")
            })?;
        let script_content = response.bytes().await?;
        fs_err::tokio::write(&script_path, &script_content).await?;

        // Set permissions
        let mut perms = fs_err::tokio::metadata(&script_path).await?.permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o755);
        }
        fs_err::tokio::set_permissions(&script_path, perms).await?;

        let mut cmd = Cmd::new("bash", "dotnet-install.sh");
        cmd.arg(&script_path).arg("--install-dir").arg(install_dir);
        add_channel_args_unix(&mut cmd, version);

        cmd.check(true).output().await?;
        Ok(())
    }

    #[cfg(windows)]
    async fn install_dotnet_windows(
        &self,
        install_dir: &Path,
        version: Option<&str>,
    ) -> Result<()> {
        let script_url = "https://dot.net/v1/dotnet-install.ps1";
        let script_path = install_dir.join("dotnet-install.ps1");

        let response = REQWEST_CLIENT
            .get(script_url)
            .send()
            .await?
            .error_for_status()
            .with_context(|| {
                format!("failed to download dotnet install script from {script_url}")
            })?;
        let script_content = response.bytes().await?;
        fs_err::tokio::write(&script_path, &script_content).await?;

        // PowerShell invocation optimized for Windows execution policy and path handling
        let mut cmd = Cmd::new("powershell.exe", "dotnet-install.ps1");
        cmd.arg("-NoProfile")
            .arg("-ExecutionPolicy")
            .arg("Bypass")
            .arg("-NonInteractive")
            .arg("-File")
            .arg(&script_path)
            .arg("-InstallDir")
            .arg(install_dir);
        add_channel_args_windows(&mut cmd, version);

        cmd.check(true).output().await?;
        Ok(())
    }
}

pub(crate) async fn query_dotnet_version(dotnet: &Path) -> Result<Version> {
    let mut cmd = Cmd::new(dotnet, "get dotnet version");
    if let Some(parent) = dotnet.parent() {
        cmd.current_dir(parent);
    }
    let stdout = cmd.arg("--version").check(true).output().await?.stdout;
    let version_str = String::from_utf8_lossy(&stdout).trim().to_string();
    parse_dotnet_version(&version_str)
        .context(format!("Failed to parse version from: {version_str}"))
}

pub(crate) fn parse_dotnet_version(version_str: &str) -> Option<Version> {
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
        _ => false,
    }
}

fn to_dotnet_install_version(request: &LanguageRequest) -> Option<String> {
    match request {
        LanguageRequest::Any { .. } => None,
        LanguageRequest::Dotnet(req) => req.to_install_version(),
        _ => None,
    }
}

/// Helper to determine if a string looks like a full semantic version (x.y.z)
/// or a channel (x.y).
fn is_full_version(ver: &str) -> bool {
    // A version is considered "full" if semver can parse it directly
    // or if it has 3 or more components.
    // "8.0" has 2 parts -> Channel.
    // "8.0.100" has 3 parts -> Version.
    Version::parse(ver).is_ok() || ver.split('.').count() >= 3
}

fn add_channel_args_unix(cmd: &mut Cmd, version: Option<&str>) {
    if let Some(ver) = version {
        if is_full_version(ver) {
            cmd.arg("--version").arg(ver);
        } else {
            // "8.0" or "LTS" or "STS"
            cmd.arg("--channel").arg(ver);
        }
    } else {
        cmd.arg("--channel").arg("LTS");
    }
}

#[cfg(windows)]
fn add_channel_args_windows(cmd: &mut Cmd, version: Option<&str>) {
    if let Some(ver) = version {
        if is_full_version(ver) {
            cmd.arg("-Version").arg(ver);
        } else {
            cmd.arg("-Channel").arg(ver);
        }
    } else {
        cmd.arg("-Channel").arg("LTS");
    }
}

pub(crate) fn installer_from_store(store: &Store) -> DotnetInstaller {
    let dotnet_dir = store.tools_path(ToolBucket::Dotnet);
    DotnetInstaller::new(dotnet_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::languages::dotnet::DotnetRequest;
    use semver::Version;

    #[test]
    fn test_parse_dotnet_version() {
        assert_eq!(
            parse_dotnet_version("8.0.100"),
            Some(Version::new(8, 0, 100))
        );
        assert_eq!(parse_dotnet_version("10.0.1"), Some(Version::new(10, 0, 1)));
        assert_eq!(parse_dotnet_version("8.0"), Some(Version::new(8, 0, 0)));
        assert_eq!(
            parse_dotnet_version("8.0.100-preview.1"),
            Some(Version::new(8, 0, 100))
        );
        assert_eq!(parse_dotnet_version("invalid"), None);
        assert_eq!(parse_dotnet_version("8"), None);
        assert_eq!(parse_dotnet_version(""), None);
    }

    #[test]
    fn test_dotnet_executable() {
        let dir = std::path::Path::new("/test/path");
        let exe = dotnet_executable(dir);

        if cfg!(windows) {
            assert_eq!(exe, dir.join("dotnet.exe"));
        } else {
            assert_eq!(exe, dir.join("dotnet"));
        }
    }

    #[test]
    fn test_version_satisfies_request() {
        let version = Version::new(8, 0, 100);

        // Test Any request
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Any { system_only: false }
        ));
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Any { system_only: true }
        ));

        // Test Dotnet requests
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::Any)
        ));
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::Major(8))
        ));
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::Major(9))
        ));
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinor(8, 0))
        ));
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinor(8, 1))
        ));
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(8, 0, 100))
        ));
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(8, 0, 101))
        ));

        // Test non-dotnet request
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Python(crate::languages::python::PythonRequest::Any)
        ));
    }

    #[test]
    fn test_to_dotnet_install_version() {
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Any { system_only: false }),
            None
        );
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::Major(8))),
            Some("8.0".to_string())
        );
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Python(
                crate::languages::python::PythonRequest::Any
            )),
            None
        );
    }

    #[test]
    fn test_is_full_version() {
        assert!(is_full_version("8.0.100"));
        assert!(is_full_version("1.2.3"));
        assert!(!is_full_version("8.0"));
        assert!(!is_full_version("8"));
        assert!(!is_full_version("LTS"));
        assert!(!is_full_version("STS"));
    }

    #[test]
    fn test_dotnet_result() {
        let path = std::path::PathBuf::from("/usr/bin/dotnet");
        let version = Version::new(8, 0, 100);
        let result = DotnetResult::new(path.clone(), version.clone());

        assert_eq!(result.dotnet(), path.as_path());
        assert_eq!(result.version(), &version);

        let display_str = result.to_string();
        assert!(display_str.contains("/usr/bin/dotnet"));
        assert!(display_str.contains("8.0.100"));
    }

    #[test]
    fn test_display_format_error() {
        let path = std::path::PathBuf::from("/usr/bin/dotnet");
        let version = Version::new(8, 0, 100);
        let result = DotnetResult::new(path, version);

        // Test the Display implementation
        let display_str = format!("{result}");
        assert!(display_str.contains("/usr/bin/dotnet@8.0.100"));

        use std::fmt::{self, Write};
        struct FailingFormatter;
        impl Write for FailingFormatter {
            fn write_str(&mut self, _s: &str) -> fmt::Result {
                Err(fmt::Error)
            }
        }

        let mut failing_fmt = FailingFormatter;
        let write_result = write!(failing_fmt, "{result}");
        assert!(write_result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn test_add_channel_args_unix_coverage() {
        let mut cmd = Cmd::new("test", "test");
        add_channel_args_unix(&mut cmd, Some("8.0.100"));
        assert!(format!("{cmd}").contains("--version 8.0.100"));

        let mut cmd = Cmd::new("test", "test");
        add_channel_args_unix(&mut cmd, Some("8.0"));
        assert!(format!("{cmd}").contains("--channel 8.0"));

        let mut cmd = Cmd::new("test", "test");
        add_channel_args_unix(&mut cmd, None);
        assert!(format!("{cmd}").contains("--channel LTS"));
    }

    #[cfg(windows)]
    #[test]
    fn test_add_channel_args_windows_coverage() {
        let mut cmd = Cmd::new("test", "test");
        add_channel_args_windows(&mut cmd, Some("8.0.100"));
        assert!(format!("{cmd}").contains("-Version 8.0.100"));

        let mut cmd = Cmd::new("test", "test");
        add_channel_args_windows(&mut cmd, Some("8.0"));
        assert!(format!("{cmd}").contains("-Channel 8.0"));

        let mut cmd = Cmd::new("test", "test");
        add_channel_args_windows(&mut cmd, None);
        assert!(format!("{cmd}").contains("-Channel LTS"));
    }
}
