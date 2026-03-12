use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use semver::Version;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::dotnet::DotnetRequest;
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Dotnet;

/// Query the version of a dotnet executable.
async fn query_dotnet_version(dotnet: &Path) -> Result<Version> {
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
fn parse_dotnet_version(version_str: &str) -> Option<Version> {
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

fn tools_path(env_path: &Path) -> PathBuf {
    env_path.join("tools")
}

fn to_dotnet_request(request: &LanguageRequest) -> Option<String> {
    match request {
        LanguageRequest::Any { .. } => None,
        LanguageRequest::Dotnet(req) => req.to_install_version(),
        _ => None,
    }
}

impl LanguageImpl for Dotnet {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %info.env_path.display(), "Installing dotnet environment");

        // Install or find dotnet SDK
        let dotnet_path = Self::install_or_find_dotnet(store, &hook.language_request)
            .await
            .context("Failed to install or find dotnet SDK")?;

        // Install additional dependencies as dotnet tools
        let tool_path = tools_path(&info.env_path);
        fs_err::tokio::create_dir_all(&tool_path).await?;

        // Build and install if repo has a .csproj or .fsproj file
        if let Some(repo_path) = hook.repo_path() {
            if has_project_file(repo_path) {
                debug!(%hook, "Packing and installing dotnet tool from repo");
                pack_and_install_local_tool(&dotnet_path, repo_path, &tool_path).await?;
            }
        }

        // Install additional dependencies as tools
        for dep in &hook.additional_dependencies {
            install_tool(&dotnet_path, &tool_path, dep).await?;
        }

        info.with_language_version(version)
            .with_toolchain(dotnet_path);
        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current_version = query_dotnet_version(&info.toolchain)
            .await
            .context("Failed to query current dotnet info")?;

        // Only check major.minor for compatibility
        if current_version.major != info.language_version.major
            || current_version.minor != info.language_version.minor
        {
            anyhow::bail!(
                "dotnet version mismatch: expected `{}.{}`, found `{}.{}`",
                info.language_version.major,
                info.language_version.minor,
                current_version.major,
                current_version.minor
            );
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Dotnet must have env path");
        let tool_path = tools_path(env_dir);
        let dotnet_path = hook
            .install_info()
            .expect("Dotnet must have install info")
            .toolchain
            .parent()
            .expect("dotnet executable must have parent");

        // Prepend both dotnet and tools to PATH
        let new_path = prepend_paths(&[&tool_path, dotnet_path]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&new_path))?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run dotnet hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::DOTNET_ROOT, dotnet_path)
                .envs(&hook.env)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

        reporter.on_run_complete(progress);

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}

impl Dotnet {
    /// Install or find dotnet SDK based on the language request.
    async fn install_or_find_dotnet(
        store: &Store,
        language_request: &LanguageRequest,
    ) -> Result<PathBuf> {
        let version_request = to_dotnet_request(language_request);

        // First, try to find a system dotnet that satisfies the request
        if let Ok(system_dotnet) = which::which("dotnet") {
            if let Ok(version) = query_dotnet_version(&system_dotnet).await {
                if Self::version_satisfies_request(&version, language_request) {
                    debug!("Using system dotnet at {}", system_dotnet.display());
                    return Ok(system_dotnet);
                }
            }
        }

        // Check if we have a managed installation that satisfies the request
        let dotnet_dir = store.tools_path(ToolBucket::Dotnet);
        if dotnet_dir.exists() {
            let dotnet_exe = dotnet_executable(&dotnet_dir);
            if dotnet_exe.exists() {
                if let Ok(version) = query_dotnet_version(&dotnet_exe).await {
                    if Self::version_satisfies_request(&version, language_request) {
                        debug!("Using managed dotnet at {}", dotnet_exe.display());
                        return Ok(dotnet_exe);
                    }
                }
            }
        }

        // If system_only is requested and we didn't find a matching system version, fail
        if matches!(language_request, LanguageRequest::Any { system_only: true }) {
            anyhow::bail!("No system dotnet installation found");
        }

        // Install dotnet SDK
        debug!("Installing dotnet SDK");
        Self::install_dotnet_sdk(store, version_request.as_deref()).await?;

        let dotnet_exe = dotnet_executable(&dotnet_dir);
        if !dotnet_exe.exists() {
            anyhow::bail!(
                "dotnet installation failed: executable not found at {}",
                dotnet_exe.display()
            );
        }

        Ok(dotnet_exe)
    }

    fn version_satisfies_request(version: &Version, request: &LanguageRequest) -> bool {
        match request {
            LanguageRequest::Any { .. } => true,
            LanguageRequest::Dotnet(req) => {
                // Create a temporary InstallInfo-like check
                match req {
                    DotnetRequest::Any => true,
                    DotnetRequest::Major(major) => version.major == *major,
                    DotnetRequest::MajorMinor(major, minor) => {
                        version.major == *major && version.minor == *minor
                    }
                    DotnetRequest::MajorMinorPatch(major, minor, patch) => {
                        version.major == *major
                            && version.minor == *minor
                            && version.patch == *patch
                    }
                }
            }
            _ => true,
        }
    }

    /// Install dotnet SDK using the official install script.
    async fn install_dotnet_sdk(store: &Store, version: Option<&str>) -> Result<()> {
        let dotnet_dir = store.tools_path(ToolBucket::Dotnet);
        fs_err::tokio::create_dir_all(&dotnet_dir).await?;

        #[cfg(unix)]
        {
            Self::install_dotnet_unix(&dotnet_dir, version).await
        }

        #[cfg(windows)]
        {
            Self::install_dotnet_windows(&dotnet_dir, version).await
        }
    }

    #[cfg(unix)]
    async fn install_dotnet_unix(dotnet_dir: &Path, version: Option<&str>) -> Result<()> {
        // Download the install script
        let script_url = "https://dot.net/v1/dotnet-install.sh";
        let script_path = dotnet_dir.join("dotnet-install.sh");

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
        cmd.arg(&script_path).arg("--install-dir").arg(dotnet_dir);

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
    async fn install_dotnet_windows(dotnet_dir: &Path, version: Option<&str>) -> Result<()> {
        // Download the install script
        let script_url = "https://dot.net/v1/dotnet-install.ps1";
        let script_path = dotnet_dir.join("dotnet-install.ps1");

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
            .arg(dotnet_dir);

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

fn dotnet_executable(dotnet_dir: &Path) -> PathBuf {
    if cfg!(windows) {
        dotnet_dir.join("dotnet.exe")
    } else {
        dotnet_dir.join("dotnet")
    }
}

/// Check if the repo contains a .csproj or .fsproj file.
fn has_project_file(repo_path: &Path) -> bool {
    std::fs::read_dir(repo_path)
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| {
            entry
                .path()
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext == "csproj" || ext == "fsproj")
        })
}

/// Pack and install a local dotnet tool from the repository.
async fn pack_and_install_local_tool(
    dotnet: &Path,
    repo_path: &Path,
    tool_path: &Path,
) -> Result<()> {
    let pack_output = tempfile::tempdir()?;

    Cmd::new(dotnet, "dotnet pack")
        .current_dir(repo_path)
        .arg("pack")
        .arg("-c")
        .arg("Release")
        .arg("-o")
        .arg(pack_output.path())
        .check(true)
        .output()
        .await
        .context("Failed to pack dotnet tool")?;

    // Find the .nupkg file
    let nupkg = std::fs::read_dir(pack_output.path())?
        .flatten()
        .find(|entry| entry.path().extension().is_some_and(|ext| ext == "nupkg"))
        .context("No .nupkg file found after packing")?;

    // Extract package name from nupkg filename (e.g., "MyTool.1.0.0.nupkg" -> "MyTool")
    let nupkg_path = nupkg.path();
    let filename = nupkg_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");

    // Package name is everything before the version number
    let package_name = filename
        .split('.')
        .take_while(|part| part.chars().next().is_some_and(|c| !c.is_ascii_digit()))
        .collect::<Vec<_>>()
        .join(".");

    if package_name.is_empty() {
        anyhow::bail!("Could not determine package name from nupkg: {filename}");
    }

    Cmd::new(dotnet, "dotnet tool install local")
        .arg("tool")
        .arg("install")
        .arg("--tool-path")
        .arg(tool_path)
        .arg("--add-source")
        .arg(pack_output.path())
        .arg(&package_name)
        .check(true)
        .output()
        .await
        .context("Failed to install local dotnet tool")?;

    Ok(())
}

/// Install a dotnet tool as an additional dependency.
///
/// The dependency can be specified as:
/// - `package` - installs latest version
/// - `package@version` - installs specific version
async fn install_tool(dotnet: &Path, tool_path: &Path, dependency: &str) -> Result<()> {
    // Normalize `:` to `@` (`:` is pre-commit convention)
    let dependency = dependency.replace(':', "@");
    let (package, version) = dependency
        .split_once('@')
        .map_or((dependency.as_str(), None), |(pkg, ver)| (pkg, Some(ver)));

    let mut cmd = Cmd::new(dotnet, "dotnet tool install");
    cmd.arg("tool")
        .arg("install")
        .arg("--tool-path")
        .arg(tool_path)
        .arg(package);

    if let Some(ver) = version {
        cmd.arg("--version").arg(ver);
    }

    cmd.check(true)
        .output()
        .await
        .with_context(|| format!("Failed to install dotnet tool: {dependency}"))?;

    Ok(())
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
