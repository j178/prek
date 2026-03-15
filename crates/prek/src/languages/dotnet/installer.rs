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

        self.verify_and_query_installation().await
    }

    /// Verify that dotnet was installed and query its version.
    async fn verify_and_query_installation(&self) -> Result<DotnetResult> {
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
        let system_dotnet = which::which("dotnet").ok();
        self.find_system_dotnet_at(system_dotnet.as_deref(), request)
            .await
    }

    async fn find_system_dotnet_at(
        &self,
        system_dotnet: Option<&std::path::Path>,
        request: &LanguageRequest,
    ) -> Result<Option<DotnetResult>> {
        let Some(system_dotnet) = system_dotnet else {
            return Ok(None);
        };

        let Ok(version) = query_dotnet_version(system_dotnet).await else {
            return Ok(None);
        };

        if version_satisfies_request(&version, request) {
            Ok(Some(DotnetResult::new(
                system_dotnet.to_path_buf(),
                version,
            )))
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

        let response = REQWEST_CLIENT
            .get(script_url)
            .send()
            .await
            .with_context(|| format!("Failed to download dotnet-install.sh from {script_url}"))?;

        if !response.status().is_success() {
            bail!(
                "Failed to download dotnet-install.sh: server returned status {}",
                response.status()
            );
        }

        let script_content = response
            .bytes()
            .await
            .context("Failed to read dotnet-install.sh response body")?;
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
        add_channel_args_unix(&mut cmd, version);

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

        let response = REQWEST_CLIENT
            .get(script_url)
            .send()
            .await
            .context("Failed to download dotnet-install.ps1")?;

        if !response.status().is_success() {
            bail!(
                "Failed to download dotnet-install.ps1: server returned status {}",
                response.status()
            );
        }

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

        add_channel_args_windows(&mut cmd, version);

        cmd.check(true)
            .output()
            .await
            .context("Failed to run dotnet-install.ps1")?;

        Ok(())
    }
}

/// Query the version of a dotnet executable.
pub(crate) async fn query_dotnet_version(dotnet: &Path) -> Result<Version> {
    let mut cmd = Cmd::new(dotnet, "get dotnet version");

    if let Some(parent) = dotnet.parent() {
        cmd.current_dir(parent);
    }

    let stdout = cmd.arg("--version").check(true).output().await?.stdout;

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

/// Add channel/version arguments to the Unix install command.
fn add_channel_args_unix(cmd: &mut Cmd, version: Option<&str>) {
    if let Some(ver) = version {
        if Version::parse(ver).is_ok() {
            cmd.arg("--version").arg(ver);
        } else {
            cmd.arg("--channel").arg(ver);
        }
    } else {
        // Default to LTS
        cmd.arg("--channel").arg("LTS");
    }
}

/// Add channel arguments to the Windows install command.
#[cfg(any(windows, test))]
fn add_channel_args_windows(cmd: &mut Cmd, version: Option<&str>) {
    if let Some(ver) = version {
        if Version::parse(ver).is_ok() {
            cmd.arg("-Version").arg(ver);
        } else {
            cmd.arg("-Channel").arg(ver);
        }
    } else {
        // Default to LTS
        cmd.arg("-Channel").arg("LTS");
    }
}

/// Create a `DotnetInstaller` from the store.
pub(crate) fn installer_from_store(store: &Store) -> DotnetInstaller {
    let dotnet_dir = store.tools_path(ToolBucket::Dotnet);
    DotnetInstaller::new(dotnet_dir)
}

#[cfg(test)]
mod tests {
    use tempfile::TempDir;

    use super::*;
    use crate::languages::dotnet::DotnetRequest;

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

    #[test]
    fn test_parse_single_number_version() {
        // Single number should fail (needs at least major.minor)
        assert!(parse_dotnet_version("8").is_none());
    }

    #[test]
    fn test_dotnet_result_display() {
        let result = DotnetResult::new(
            PathBuf::from("/usr/share/dotnet/dotnet"),
            Version::new(8, 0, 100),
        );
        assert_eq!(format!("{result}"), "/usr/share/dotnet/dotnet@8.0.100");
    }

    #[test]
    fn test_dotnet_result_accessors() {
        let result = DotnetResult::new(
            PathBuf::from("/usr/share/dotnet/dotnet"),
            Version::new(9, 0, 100),
        );
        assert_eq!(result.dotnet(), Path::new("/usr/share/dotnet/dotnet"));
        assert_eq!(result.version(), &Version::new(9, 0, 100));
    }

    #[test]
    fn test_dotnet_executable_unix() {
        #[cfg(unix)]
        {
            let path = dotnet_executable(Path::new("/opt/dotnet"));
            assert_eq!(path, PathBuf::from("/opt/dotnet/dotnet"));
        }
    }

    #[test]
    fn test_version_satisfies_request_any() {
        let version = Version::new(8, 0, 100);

        // LanguageRequest::Any should always match
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Any { system_only: false }
        ));
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Any { system_only: true }
        ));
    }

    #[test]
    fn test_version_satisfies_request_dotnet_any() {
        let version = Version::new(8, 0, 100);
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::Any)
        ));
    }

    #[test]
    fn test_version_satisfies_request_major() {
        let version = Version::new(8, 0, 100);

        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::Major(8))
        ));
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::Major(9))
        ));
    }

    #[test]
    fn test_version_satisfies_request_major_minor() {
        let version = Version::new(8, 0, 100);

        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinor(8, 0))
        ));
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinor(8, 1))
        ));
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinor(9, 0))
        ));
    }

    #[test]
    fn test_version_satisfies_request_major_minor_patch() {
        let version = Version::new(8, 0, 100);

        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(8, 0, 100))
        ));
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(8, 0, 101))
        ));
        assert!(!version_satisfies_request(
            &version,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(8, 1, 100))
        ));
    }

    #[test]
    fn test_version_satisfies_request_other_language() {
        // Other language requests should return true (fallback case)
        let version = Version::new(8, 0, 100);
        assert!(version_satisfies_request(
            &version,
            &LanguageRequest::Python(crate::languages::python::PythonRequest::Any)
        ));
    }

    #[test]
    fn test_to_dotnet_install_version_any() {
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Any { system_only: false }),
            None
        );
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::Any)),
            None
        );
    }

    #[test]
    fn test_to_dotnet_install_version_major() {
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::Major(8))),
            Some("8.0".to_string())
        );
    }

    #[test]
    fn test_to_dotnet_install_version_major_minor() {
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::MajorMinor(9, 0))),
            Some("9.0".to_string())
        );
    }

    #[test]
    fn test_to_dotnet_install_version_major_minor_patch() {
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(
                8, 0, 100
            ))),
            Some("8.0.100".to_string())
        );
    }

    #[test]
    fn test_to_dotnet_install_version_other_language() {
        // Other language requests should return None
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Python(
                crate::languages::python::PythonRequest::Any
            )),
            None
        );
    }

    #[test]
    fn test_dotnet_installer_new() {
        let root = PathBuf::from("/test/root");
        let installer = DotnetInstaller::new(root.clone());
        assert_eq!(installer.root, root);
    }

    #[tokio::test]
    async fn test_find_installed_no_executable() {
        let temp = TempDir::new().unwrap();
        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // No dotnet executable exists, should return None
        let result = installer
            .find_installed(&LanguageRequest::Any { system_only: false })
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_find_installed_with_invalid_executable() {
        let temp = TempDir::new().unwrap();
        let dotnet_path = dotnet_executable(temp.path());

        // Create a fake dotnet executable that outputs invalid version
        #[cfg(unix)]
        {
            fs_err::write(&dotnet_path, "#!/bin/sh\necho 'invalid'").unwrap();
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs_err::metadata(&dotnet_path).unwrap().permissions();
            perms.set_mode(0o755);
            fs_err::set_permissions(&dotnet_path, perms).unwrap();
        }
        #[cfg(windows)]
        {
            // On Windows, we can't easily create a fake executable that runs
            // so we just verify the path logic
            fs_err::write(&dotnet_path, "invalid").unwrap();
        }

        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // Executable exists but returns invalid version, should return None
        let result = installer
            .find_installed(&LanguageRequest::Any { system_only: false })
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_find_installed_version_mismatch() {
        let temp = TempDir::new().unwrap();
        let dotnet_path = dotnet_executable(temp.path());

        // Create a fake dotnet executable that outputs version 7.0.100
        fs_err::write(&dotnet_path, "#!/bin/sh\necho '7.0.100'").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs_err::metadata(&dotnet_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs_err::set_permissions(&dotnet_path, perms).unwrap();

        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // Request version 8, but installed is 7 - should return None
        let result = installer
            .find_installed(&LanguageRequest::Dotnet(DotnetRequest::Major(8)))
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_find_installed_version_matches() {
        let temp = TempDir::new().unwrap();
        let dotnet_path = dotnet_executable(temp.path());

        // Create a fake dotnet executable that outputs version 8.0.100
        fs_err::write(&dotnet_path, "#!/bin/sh\necho '8.0.100'").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs_err::metadata(&dotnet_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs_err::set_permissions(&dotnet_path, perms).unwrap();

        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // Request version 8, installed is 8.0.100 - should return Some
        let result = installer
            .find_installed(&LanguageRequest::Dotnet(DotnetRequest::Major(8)))
            .await
            .unwrap();
        assert!(result.is_some());
        let dotnet_result = result.unwrap();
        assert_eq!(dotnet_result.version(), &Version::new(8, 0, 100));
    }

    #[tokio::test]
    async fn test_find_system_dotnet_not_found() {
        // When `which dotnet` fails, should return None
        // This test relies on dotnet not being in the path for the test environment
        // or we just verify the logic by testing the version check paths

        let temp = TempDir::new().unwrap();
        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // find_system_dotnet returns None when which::which fails
        // We can't control `which` easily, but we can verify the function doesn't panic
        let _result = installer
            .find_system_dotnet(&LanguageRequest::Any { system_only: false })
            .await;
    }

    #[tokio::test]
    async fn test_install_system_only_no_system_dotnet() {
        let temp = TempDir::new().unwrap();
        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // With system_only=true, if no system dotnet is found, should fail
        // Note: This test may pass or fail depending on whether dotnet is installed
        // We primarily verify the error message format when system dotnet isn't available
        let request = LanguageRequest::Any { system_only: true };
        let result = installer.install(&request, false).await;

        // If no system dotnet is installed, this should error
        // The error should mention "No system dotnet installation found"
        if let Err(err) = result {
            let err_msg = err.to_string();
            assert!(
                err_msg.contains("No system dotnet installation found")
                    || err_msg.contains("No suitable dotnet version found"),
                "Unexpected error: {err_msg}"
            );
        }
        // If system dotnet IS installed, result will be Ok - that's also valid
    }

    #[tokio::test]
    async fn test_install_downloads_disabled() {
        let temp = TempDir::new().unwrap();
        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // Request a specific version that's unlikely to be installed
        // with downloads disabled - should fail
        let request = LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(99, 99, 999));
        let result = installer.install(&request, false).await;

        // Should fail because no matching version and downloads are disabled
        assert!(result.is_err());
        let err_msg = result.unwrap_err().to_string();
        assert!(
            err_msg.contains("No suitable dotnet version found and downloads are disabled"),
            "Unexpected error: {err_msg}"
        );
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_install_uses_managed_when_version_matches() {
        let temp = TempDir::new().unwrap();
        let dotnet_path = dotnet_executable(temp.path());

        // Create a fake dotnet executable that outputs version 8.0.100
        fs_err::write(&dotnet_path, "#!/bin/sh\necho '8.0.100'").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs_err::metadata(&dotnet_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs_err::set_permissions(&dotnet_path, perms).unwrap();

        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // Request version 8 specifically - if system has different major version,
        // it should use the managed dotnet
        let result = installer
            .install(&LanguageRequest::Dotnet(DotnetRequest::Major(8)), false)
            .await;

        // Should succeed - either system dotnet 8.x or managed 8.0.100
        assert!(result.is_ok());
        let dotnet_result = result.unwrap();
        assert_eq!(dotnet_result.version().major, 8);
    }

    #[tokio::test]
    #[cfg(unix)]
    async fn test_find_installed_returns_matching_version() {
        // This test specifically exercises the find_installed path
        // which is covered when install() finds a managed installation
        let temp = TempDir::new().unwrap();
        let dotnet_path = dotnet_executable(temp.path());

        // Create a managed dotnet that outputs version 8.0.100
        fs_err::write(&dotnet_path, "#!/bin/sh\necho '8.0.100'").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs_err::metadata(&dotnet_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs_err::set_permissions(&dotnet_path, perms).unwrap();

        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // Directly test find_installed - this covers the "Using managed dotnet" branch
        let result = installer
            .find_installed(&LanguageRequest::Dotnet(DotnetRequest::Major(8)))
            .await
            .unwrap();

        assert!(result.is_some());
        let dotnet_result = result.unwrap();
        assert_eq!(dotnet_result.version(), &Version::new(8, 0, 100));
        assert_eq!(dotnet_result.dotnet(), dotnet_path);
    }

    #[test]
    fn test_dotnet_executable_path() {
        let base_path = Path::new("/opt/dotnet");
        let exe_path = dotnet_executable(base_path);

        #[cfg(unix)]
        assert_eq!(exe_path, PathBuf::from("/opt/dotnet/dotnet"));

        #[cfg(windows)]
        assert_eq!(exe_path, PathBuf::from("/opt/dotnet/dotnet.exe"));
    }

    #[test]
    fn test_version_satisfies_request_all_branches() {
        let v8_0_100 = Version::new(8, 0, 100);
        let v8_1_0 = Version::new(8, 1, 0);
        let v9_0_0 = Version::new(9, 0, 0);

        // Test LanguageRequest::Any with both system_only values
        assert!(version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Any { system_only: false }
        ));
        assert!(version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Any { system_only: true }
        ));

        // Test DotnetRequest::Any
        assert!(version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Dotnet(DotnetRequest::Any)
        ));

        // Test Major matching and non-matching
        assert!(version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Dotnet(DotnetRequest::Major(8))
        ));
        assert!(!version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Dotnet(DotnetRequest::Major(9))
        ));

        // Test MajorMinor matching and non-matching
        assert!(version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinor(8, 0))
        ));
        assert!(!version_satisfies_request(
            &v8_1_0,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinor(8, 0))
        ));
        assert!(!version_satisfies_request(
            &v9_0_0,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinor(8, 0))
        ));

        // Test MajorMinorPatch matching and non-matching
        assert!(version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(8, 0, 100))
        ));
        assert!(!version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(8, 0, 101))
        ));
        assert!(!version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(8, 1, 100))
        ));
        assert!(!version_satisfies_request(
            &v8_0_100,
            &LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(9, 0, 100))
        ));
    }

    #[test]
    fn test_to_dotnet_install_version_all_branches() {
        // LanguageRequest::Any returns None
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Any { system_only: false }),
            None
        );
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Any { system_only: true }),
            None
        );

        // DotnetRequest::Any returns None
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::Any)),
            None
        );

        // Major returns "X.0"
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::Major(8))),
            Some("8.0".to_string())
        );
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::Major(9))),
            Some("9.0".to_string())
        );

        // MajorMinor returns "X.Y"
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::MajorMinor(8, 0))),
            Some("8.0".to_string())
        );
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::MajorMinor(9, 1))),
            Some("9.1".to_string())
        );

        // MajorMinorPatch returns "X.Y.Z"
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Dotnet(DotnetRequest::MajorMinorPatch(
                8, 0, 100
            ))),
            Some("8.0.100".to_string())
        );

        // Other language requests return None (fallback branch)
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Python(
                crate::languages::python::PythonRequest::Any
            )),
            None
        );
        assert_eq!(
            to_dotnet_install_version(&LanguageRequest::Node(
                crate::languages::node::NodeRequest::Any
            )),
            None
        );
    }

    #[test]
    fn test_parse_dotnet_version_edge_cases() {
        // Valid versions
        assert_eq!(
            parse_dotnet_version("8.0.100"),
            Some(Version::new(8, 0, 100))
        );
        assert_eq!(parse_dotnet_version("8.0"), Some(Version::new(8, 0, 0)));
        assert_eq!(
            parse_dotnet_version("9.0.100-preview.1"),
            Some(Version::new(9, 0, 100))
        );

        // Invalid versions
        assert!(parse_dotnet_version("").is_none());
        assert!(parse_dotnet_version("8").is_none());
        assert!(parse_dotnet_version("invalid").is_none());
        assert!(parse_dotnet_version("a.b.c").is_none());
        assert!(parse_dotnet_version("8.b.100").is_none());
    }

    #[test]
    fn test_add_channel_args_unix_with_version() {
        let mut cmd = crate::process::Cmd::new("bash", "test");
        add_channel_args_unix(&mut cmd, Some("8.0"));
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"--channel".to_string()));
        assert!(args.contains(&"8.0".to_string()));
    }

    #[test]
    fn test_add_channel_args_unix_without_version() {
        let mut cmd = crate::process::Cmd::new("bash", "test");
        add_channel_args_unix(&mut cmd, None);
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"--channel".to_string()));
        assert!(args.contains(&"LTS".to_string()));
    }

    #[test]
    fn test_add_channel_args_windows_with_version() {
        let mut cmd = crate::process::Cmd::new("powershell", "test");
        add_channel_args_windows(&mut cmd, Some("8.0"));
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"-Channel".to_string()));
        assert!(args.contains(&"8.0".to_string()));
    }

    #[test]
    fn test_add_channel_args_windows_without_version() {
        let mut cmd = crate::process::Cmd::new("powershell", "test");
        add_channel_args_windows(&mut cmd, None);
        let args: Vec<_> = cmd
            .get_args()
            .map(|s| s.to_string_lossy().to_string())
            .collect();
        assert!(args.contains(&"-Channel".to_string()));
        assert!(args.contains(&"LTS".to_string()));
    }

    #[tokio::test]
    async fn test_verify_installation_fails_when_executable_not_found() {
        // Test verify_and_query_installation with missing executable
        let temp = TempDir::new().unwrap();
        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        // Call the verification method on an empty directory (no dotnet installed)
        let result = installer.verify_and_query_installation().await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("executable not found"),
            "expected 'executable not found' error, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_find_system_dotnet_at_returns_none_when_path_is_none() {
        let temp = TempDir::new().unwrap();
        let installer = DotnetInstaller::new(temp.path().to_path_buf());

        let request = LanguageRequest::Any { system_only: false };
        // Pass None to simulate dotnet not being in PATH
        let result = installer.find_system_dotnet_at(None, &request).await;

        assert!(result.is_ok());
        assert!(result.unwrap().is_none());
    }
}
