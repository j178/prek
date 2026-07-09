use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use reqwest::Url;
use serde::Deserialize;
use target_lexicon::{Architecture, Environment, HOST, OperatingSystem, Triple};
use tracing::{debug, trace, warn};

use crate::archive;
use crate::checksum::{Sha256Digest, digest_from_sha256sums};
use crate::fs::LockedFile;
use crate::http::{DownloadChecksumPolicy, REQWEST_CLIENT, download_artifact_with};
use crate::languages::ruby::RubyRequest;
use crate::process::Cmd;
use crate::store::Store;

const RV_RUBY_DEFAULT_URL: &str = "https://github.com/spinel-coop/rv-ruby";

fn rv_ruby_base_url(env_vars: &impl EnvVarsRead) -> String {
    match env_vars.var(EnvVars::PREK_RUBY_MIRROR) {
        Ok(mirror) => mirror.trim_end_matches('/').to_string(),
        Err(_) => RV_RUBY_DEFAULT_URL.to_string(),
    }
}

/// Returns a URL compatible with the GitHub Releases API for listing rv-ruby
/// versions, and whether the target host is github.com (for auth token
/// decisions).
///
/// When the mirror is a `github.com` URL, the path is rewritten to use the
/// `api.github.com` host (e.g. `https://github.com/org/repo` becomes
/// `https://api.github.com/repos/org/repo/releases/latest`).
fn rv_ruby_api_url(env_vars: &impl EnvVarsRead) -> (String, bool) {
    let base = rv_ruby_base_url(env_vars);
    if let Some(path) = github_repo_path(&base) {
        (
            format!("https://api.github.com/repos{path}/releases/latest"),
            true,
        )
    } else {
        (format!("{base}/releases/latest"), false)
    }
}

fn github_repo_path(url: &str) -> Option<String> {
    let url = Url::parse(url).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || !url.username().is_empty()
        || url.password().is_some()
        || !matches!(url.port(), None | Some(443))
    {
        return None;
    }

    let segments = url.path_segments()?.collect::<Vec<_>>();
    let [owner, repo] = segments.as_slice() else {
        return None;
    };
    if owner.is_empty() || repo.is_empty() {
        return None;
    }

    Some(format!("/{owner}/{repo}"))
}

/// Conditionally add a GitHub auth token to a request builder.
/// Only sends `GITHUB_TOKEN` when `is_github` is true.
fn maybe_add_github_auth(
    req: reqwest::RequestBuilder,
    is_github: bool,
    env_vars: &impl EnvVarsRead,
) -> reqwest::RequestBuilder {
    if is_github {
        if let Ok(token) = env_vars.var(EnvVars::GITHUB_TOKEN) {
            return req.bearer_auth(token);
        }
    }
    req
}

#[derive(Deserialize)]
struct GitHubRelease {
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
}

/// Returns the rv-ruby release asset platform suffix for the current target.
///
/// These strings must match the asset filenames published by rv-ruby
/// (e.g. `ruby-3.4.8.arm64_linux_musl.tar.gz`). The canonical source is
/// `HostPlatform::ruby_arch_str()` in rv's `rv-platform` crate:
/// <https://github.com/spinel-coop/rv/blob/main/crates/rv-platform/src/lib.rs>
///
/// The macOS names (`ventura`, `arm64_sonoma`) are Homebrew bottle tags currently
/// pinned by rv-ruby's packaging script. rv currently build using macOS 15 on Intel
/// which would suggest a 'sequoia' tag, but their packaging script currently renames the
/// output to 'ventura'. If this ever changes, this mapping will need to be updated
/// accordingly.
fn rv_platform_string(triple: &Triple) -> Option<&'static str> {
    match (
        triple.operating_system,
        triple.architecture,
        triple.environment,
    ) {
        // macOS
        (OperatingSystem::Darwin(_), Architecture::X86_64, _) => Some("ventura"),
        (OperatingSystem::Darwin(_), Architecture::Aarch64(_), _) => Some("arm64_sonoma"),

        // Linux glibc
        (OperatingSystem::Linux, Architecture::X86_64, Environment::Gnu) => Some("x86_64_linux"),
        (OperatingSystem::Linux, Architecture::Aarch64(_), Environment::Gnu) => Some("arm64_linux"),

        // Linux musl (Alpine)
        (OperatingSystem::Linux, Architecture::X86_64, Environment::Musl) => {
            Some("x86_64_linux_musl")
        }
        (OperatingSystem::Linux, Architecture::Aarch64(_), Environment::Musl) => {
            Some("arm64_linux_musl")
        }

        // unsupported OS/CPU/libc combination
        _ => None,
    }
}

/// Result of finding/installing a Ruby interpreter
#[derive(Debug)]
pub(crate) struct RubyResult {
    /// Path to ruby executable
    ruby_bin: PathBuf,

    /// Path to gem executable for this Ruby
    gem_bin: PathBuf,

    /// Ruby version
    version: semver::Version,
}

impl RubyResult {
    fn from_managed_dir(dir: &Path, version: semver::Version) -> Result<Self> {
        let bin_dir = dir.join("bin");
        let ruby_bin = bin_dir.join("ruby").with_extension(EXE_EXTENSION);
        let gem_bin = find_gem_for_ruby(&ruby_bin)?;

        Ok(Self {
            ruby_bin,
            gem_bin,
            version,
        })
    }

    async fn from_executable(ruby_bin: PathBuf) -> Result<Self> {
        let gem_bin = find_gem_for_ruby(&ruby_bin)?;
        let version = query_ruby_version(&ruby_bin).await?;

        Ok(Self {
            ruby_bin,
            gem_bin,
            version,
        })
    }

    pub(crate) fn ruby_bin(&self) -> &Path {
        &self.ruby_bin
    }

    pub(crate) fn gem_bin(&self) -> &Path {
        &self.gem_bin
    }

    pub(crate) fn version(&self) -> &semver::Version {
        &self.version
    }
}

/// Ruby installer that finds or installs Ruby interpreters
pub(crate) struct RubyInstaller {
    root: PathBuf,
}

impl RubyInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Main installation entry point
    pub(crate) async fn install(
        &self,
        store: &Store,
        request: &RubyRequest,
        allows_download: bool,
    ) -> Result<RubyResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;
        let _lock = LockedFile::acquire(self.root.join(".lock"), "ruby").await?;

        // 1. Check previously downloaded rubies
        if let Some(ruby) = self.find_installed(request) {
            trace!(
                "Using managed Ruby: {} at {}",
                ruby.version(),
                ruby.ruby_bin().display()
            );
            return Ok(ruby);
        }

        // 2. Check system Ruby (PATH + version managers)
        if let Some(ruby) = self.find_system_ruby(request).await? {
            trace!(
                "Using system Ruby: {} at {}",
                ruby.version(),
                ruby.ruby_bin().display()
            );
            return Ok(ruby);
        }

        // 3. Download if allowed and platform is supported
        if !allows_download {
            anyhow::bail!(ruby_not_found_error(
                request,
                // allows_download can only be false if the original request was
                // for any version of ruby, but system-only.
                "Automatic installation is disabled (language_version: system)."
            ));
        }

        let Some(platform) = rv_platform_string(&HOST) else {
            anyhow::bail!(ruby_not_found_error(
                request,
                // Windows, unknown CPU, etc. that doesn't have a matching rv-ruby
                // release asset (that we know about).
                "Automatic installation is not supported on this platform."
            ));
        };

        let versions = match self.list_remote_versions(platform).await {
            Ok(v) => v,
            Err(e) => {
                anyhow::bail!(
                    "{}\n\nCaused by:\n  {e}",
                    ruby_not_found_error(
                        request,
                        "Failed to fetch available Ruby versions from rv-ruby."
                    )
                );
            }
        };

        let Some(version) = versions.into_iter().find(|v| request.matches(v)) else {
            anyhow::bail!(ruby_not_found_error(
                request,
                &format!("No rv-ruby release found matching: {request}")
            ));
        };
        self.download(store, &version, platform).await
    }

    /// Scan `self.root` for previously downloaded Ruby versions.
    fn find_installed(&self, request: &RubyRequest) -> Option<RubyResult> {
        fs_err::read_dir(&self.root)
            .ok()?
            .flatten()
            .filter(|entry| entry.file_type().is_ok_and(|f| f.is_dir()))
            .filter_map(|entry| {
                let version = semver::Version::parse(&entry.file_name().to_string_lossy()).ok()?;
                match RubyResult::from_managed_dir(&entry.path(), version) {
                    Ok(ruby) => Some(ruby),
                    Err(err) => {
                        trace!(path = %entry.path().display(), %err, "Skipping invalid Ruby installation");
                        None
                    }
                }
            })
            .sorted_unstable_by(|a, b| b.version.cmp(&a.version)) // descending
            .find_map(|ruby| {
                if request.matches(&ruby.version) {
                    Some(ruby)
                } else {
                    None
                }
            })
    }

    /// Fetch available Ruby versions from the rv-ruby GitHub release.
    async fn list_remote_versions(&self, platform: &str) -> Result<Vec<semver::Version>> {
        let (api_url, is_github) = rv_ruby_api_url(&EnvVars);
        let suffix = format!(".{platform}.tar.gz");

        let req = REQWEST_CLIENT
            .get(&api_url)
            .header("Accept", "application/vnd.github+json");
        let req = maybe_add_github_auth(req, is_github, &EnvVars);

        let release: GitHubRelease = req
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .with_context(|| format!("Failed to fetch rv-ruby releases from {api_url}"))?
            .json()
            .await
            .context("Failed to parse rv-ruby release JSON")?;

        let versions = release
            .assets
            .iter()
            .filter_map(|asset| parse_version_from_asset(&asset.name, &suffix))
            .sorted_unstable()
            .rev()
            .collect();

        Ok(versions)
    }

    /// Download and extract a specific Ruby version from rv-ruby.
    ///
    /// Adds GitHub authentication only for GitHub-hosted sources; the download
    /// helper verifies the archive checksum before extraction.
    async fn download(
        &self,
        store: &Store,
        version: &semver::Version,
        platform: &str,
    ) -> Result<RubyResult> {
        let filename = format!("ruby-{version}.{platform}.tar.gz");
        let base_url = rv_ruby_base_url(&EnvVars);
        let is_github = github_repo_path(&base_url).is_some();
        let download_base_url = format!("{base_url}/releases/latest/download");
        let url = format!("{download_base_url}/{filename}");
        let checksum_url = format!("{download_base_url}/SHA256SUMS");

        let version_str = version.to_string();
        let target = self.root.join(&version_str);

        debug!(url = %url, target = %target.display(), "Downloading Ruby {version}");

        let download = download_artifact_with(
            &url,
            &filename,
            store,
            DownloadChecksumPolicy::from_env(&EnvVars),
            async || Self::fetch_checksum(&checksum_url, &filename, is_github).await,
            |req| maybe_add_github_auth(req, is_github, &EnvVars),
        )
        .await
        .with_context(|| format!("Failed to download Ruby {version} from {url}"))?;
        let extracted = archive::extract_archive(download.path())
            .await
            .with_context(|| format!("Failed to extract Ruby {version}"))?;
        // rv-ruby tarballs contain: rv-ruby@{version}/{version}/bin/ruby
        // After strip_component, `extracted` is the rv-ruby@{version}/ directory.
        // Move the inner {version}/ directory to our target.
        let inner = extracted.join(&version_str);
        if !inner.exists() {
            anyhow::bail!(
                "Expected directory `{version_str}` inside rv-ruby archive, found: {:?}",
                fs_err::read_dir(extracted)?
                    .flatten()
                    .map(|e| e.file_name())
                    .collect::<Vec<_>>()
            );
        }

        if target.exists() {
            debug!(target = %target.display(), "Removing existing Ruby");
            fs_err::tokio::remove_dir_all(&target).await?;
        }

        fs_err::tokio::rename(&inner, &target).await?;

        RubyResult::from_managed_dir(&target, version.clone())
    }

    async fn fetch_checksum(
        checksum_url: &str,
        filename: &str,
        is_github: bool,
    ) -> Result<Option<Sha256Digest>> {
        let req = REQWEST_CLIENT
            .get(checksum_url)
            .header("Accept", "application/octet-stream");
        let req = maybe_add_github_auth(req, is_github, &EnvVars);

        let response = req.send().await.with_context(|| {
            format!("Failed to fetch rv-ruby checksum file from {checksum_url}")
        })?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let checksums = response
            .error_for_status()
            .with_context(|| format!("Failed to fetch rv-ruby checksum file from {checksum_url}"))?
            .text()
            .await
            .with_context(|| format!("Failed to read rv-ruby checksum file from {checksum_url}"))?;

        digest_from_sha256sums(&checksums, filename)
    }

    /// Find Ruby in the system PATH
    async fn find_system_ruby(&self, request: &RubyRequest) -> Result<Option<RubyResult>> {
        // Try all rubies in PATH first
        if let Ok(ruby_paths) = which::which_all("ruby") {
            for ruby_path in ruby_paths {
                if let Some(result) = try_ruby_path(&ruby_path, request).await {
                    return Ok(Some(result));
                }
            }
        }

        // If we didn't find a suitable Ruby in PATH, search version manager directories
        #[cfg(not(target_os = "windows"))]
        if let Some(result) = search_version_managers(request).await {
            return Ok(Some(result));
        }

        Ok(None)
    }
}

/// Try to use a Ruby at the given path
async fn try_ruby_path(ruby_path: &Path, request: &RubyRequest) -> Option<RubyResult> {
    match RubyResult::from_executable(ruby_path.to_path_buf()).await {
        Ok(ruby) if request.matches(ruby.version()) => Some(ruby),
        Ok(_) => None,
        Err(err) => {
            warn!("Failed to inspect Ruby at {}: {err}", ruby_path.display());
            None
        }
    }
}

/// Search version manager directories for suitable Ruby installations
#[cfg(not(target_os = "windows"))]
async fn search_version_managers(request: &RubyRequest) -> Option<RubyResult> {
    let home = EnvVars.var(EnvVars::HOME).ok()?;
    let home_path = PathBuf::from(home);

    // Common version manager and Homebrew directories
    let search_dirs = [
        // rvm: ~/.rvm/rubies/ruby-3.4.6/bin/ruby
        home_path.join(".rvm/rubies"),
        // rv: ~/.local/share/rv/rubies/3.4.6/bin/ruby
        home_path.join(".local/share/rv/rubies"),
        // rv legacy path: ~/.data/rv/rubies/3.4.6/bin/ruby
        home_path.join(".data/rv/rubies"),
        // mise: ~/.local/share/mise/installs/ruby/3.4.6/bin/ruby
        home_path.join(".local/share/mise/installs/ruby"),
        // rbenv: ~/.rbenv/versions/3.4.6/bin/ruby
        home_path.join(".rbenv/versions"),
        // asdf: ~/.asdf/installs/ruby/3.4.6/bin/ruby
        home_path.join(".asdf/installs/ruby"),
        // chruby: ~/.rubies/ruby-3.4.6/bin/ruby
        home_path.join(".rubies"),
        // chruby system-wide: /opt/rubies/ruby-3.4.6/bin/ruby
        PathBuf::from("/opt/rubies"),
        // Homebrew (Apple Silicon): /opt/homebrew/Cellar/ruby/3.4.6/bin/ruby
        PathBuf::from("/opt/homebrew/Cellar/ruby"),
        // Homebrew (Intel): /usr/local/Cellar/ruby/3.4.6/bin/ruby
        PathBuf::from("/usr/local/Cellar/ruby"),
        // Linuxbrew: /home/linuxbrew/.linuxbrew/Cellar/ruby/3.4.6/bin/ruby
        PathBuf::from("/home/linuxbrew/.linuxbrew/Cellar/ruby"),
        // Linuxbrew (user): ~/.linuxbrew/Cellar/ruby/3.4.6/bin/ruby
        home_path.join(".linuxbrew/Cellar/ruby"),
    ];

    for search_dir in &search_dirs {
        if let Some(result) = search_ruby_installations(search_dir, request).await {
            return Some(result);
        }
    }

    None
}

/// Search a version manager directory for Ruby installations
#[cfg(not(target_os = "windows"))]
async fn search_ruby_installations(dir: &Path, request: &RubyRequest) -> Option<RubyResult> {
    let entries = fs_err::read_dir(dir).ok()?;

    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let ruby_path = path.join("bin/ruby");
        if ruby_path.exists() {
            if let Some(result) = try_ruby_path(&ruby_path, request).await {
                trace!(
                    "Found suitable Ruby in version manager: {}",
                    ruby_path.display()
                );
                return Some(result);
            }
        }
    }

    None
}

/// Extract a Ruby version from an rv-ruby release asset name.
///
/// Given suffix `.x86_64_linux.tar.gz` and asset `ruby-3.4.8.x86_64_linux.tar.gz`,
/// returns `Some(Version(3.4.8))`. Returns `None` for non-matching platforms,
/// non-semver versions (e.g. `0.49`), and pre-release versions.
fn parse_version_from_asset(name: &str, platform_suffix: &str) -> Option<semver::Version> {
    let name = name.strip_prefix("ruby-")?;
    let version_str = name.strip_suffix(platform_suffix)?;
    let version = semver::Version::parse(version_str).ok()?;
    // Skip pre-release versions (e.g. 3.5.0-preview1) unless explicitly requested
    if !version.pre.is_empty() {
        return None;
    }
    Some(version)
}

/// Generate a consistent error message for all "can't get Ruby" scenarios.
fn ruby_not_found_error(request: &RubyRequest, reason: &str) -> String {
    format!(
        "No suitable Ruby found for request: {request}\n{reason}\nPlease install Ruby manually."
    )
}

/// Find gem executable alongside Ruby
fn find_gem_for_ruby(ruby_path: &Path) -> Result<PathBuf> {
    let executable_names: &[&str] = if cfg!(windows) {
        &["gem.cmd", "gem.bat", "gem.exe"]
    } else {
        &["gem"]
    };

    for name in executable_names {
        let gem_path = ruby_path.with_file_name(name);
        if gem_path.is_file() {
            return Ok(gem_path);
        }
    }

    anyhow::bail!(
        "No gem executable found alongside Ruby at {}",
        ruby_path.display()
    )
}

/// Query the Ruby version.
pub(crate) async fn query_ruby_version(ruby_path: &Path) -> Result<semver::Version> {
    let script = "puts RUBY_VERSION";
    let output = Cmd::new(ruby_path)
        .arg("-e")
        .arg(script)
        .check(true)
        .output()
        .await?;

    let version_str = str::from_utf8(&output.stdout)?.trim_ascii();
    let version = semver::Version::parse(version_str)
        .with_context(|| format!("Failed to parse Ruby version: {version_str}"))?;

    Ok(version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs_err as fs;
    use std::str::FromStr;
    use target_lexicon::Triple;
    use tempfile::TempDir;

    fn test_gem_executable() -> &'static str {
        if cfg!(windows) { "gem.bat" } else { "gem" }
    }

    #[test]
    fn test_ruby_request_display() {
        assert_eq!(RubyRequest::Any.to_string(), "any");
        assert_eq!(RubyRequest::Exact(3, 4, 6).to_string(), "3.4.6");
        assert_eq!(RubyRequest::MajorMinor(3, 4).to_string(), "3.4");
        assert_eq!(RubyRequest::Major(3).to_string(), "3");

        let range = semver::VersionReq::parse(">=3.2").unwrap();
        assert_eq!(
            RubyRequest::Range(range, ">=3.2".to_string()).to_string(),
            ">=3.2"
        );
    }

    #[tokio::test]
    #[cfg(not(target_os = "windows"))]
    async fn test_search_ruby_installations_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let request = RubyRequest::Any;

        let result = search_ruby_installations(temp_dir.path(), &request).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    #[cfg(not(target_os = "windows"))]
    async fn test_search_ruby_installations_no_ruby() {
        let temp_dir = TempDir::new().unwrap();

        // Create a subdirectory without ruby
        let ruby_dir = temp_dir.path().join("ruby-3.4.6");
        fs::create_dir_all(ruby_dir.join("bin")).unwrap();

        let request = RubyRequest::Any;
        let result = search_ruby_installations(temp_dir.path(), &request).await;
        assert!(result.is_none());
    }

    #[tokio::test]
    #[cfg(not(target_os = "windows"))]
    async fn test_search_ruby_installations_with_file() {
        let temp_dir = TempDir::new().unwrap();

        // Create a subdirectory with a fake ruby file (not executable)
        let ruby_dir = temp_dir.path().join("ruby-3.4.6");
        fs::create_dir_all(ruby_dir.join("bin")).unwrap();
        let ruby_path = ruby_dir.join("bin/ruby");
        fs::write(&ruby_path, "#!/bin/sh\necho fake ruby").unwrap();

        let request = RubyRequest::Any;
        let result = search_ruby_installations(temp_dir.path(), &request).await;

        // Result should be None because the fake ruby won't execute properly
        // This test verifies the function handles execution failures gracefully
        assert!(result.is_none());
    }

    #[test]
    fn test_ruby_not_found_error() {
        let error = ruby_not_found_error(&RubyRequest::Exact(3, 4, 6), "Some reason.");
        assert!(error.contains("3.4.6"));
        assert!(error.contains("No suitable Ruby found"));
        assert!(error.contains("Some reason."));
        assert!(error.contains("Please install Ruby manually."));

        let error = ruby_not_found_error(&RubyRequest::Any, "Another reason.");
        assert!(error.contains("any"));
        assert!(error.contains("Another reason."));
    }

    #[test]
    fn test_find_gem_for_ruby_missing() {
        let temp_dir = TempDir::new().unwrap();
        let ruby_path = temp_dir
            .path()
            .join("bin/ruby")
            .with_extension(EXE_EXTENSION);

        // Create parent dir but no gem
        fs::create_dir_all(temp_dir.path().join("bin")).unwrap();
        fs::write(&ruby_path, "fake").unwrap();

        let result = find_gem_for_ruby(&ruby_path);
        assert!(result.is_err());
        assert!(
            result
                .unwrap_err()
                .to_string()
                .contains("No gem executable found")
        );
    }

    #[test]
    fn test_find_gem_for_ruby_found() {
        let temp_dir = TempDir::new().unwrap();
        let bin_dir = temp_dir.path().join("bin");
        fs::create_dir_all(&bin_dir).unwrap();

        let ruby_path = bin_dir.join("ruby").with_extension(EXE_EXTENSION);
        let gem_path = bin_dir.join(test_gem_executable());

        fs::write(&ruby_path, "fake ruby").unwrap();
        fs::write(&gem_path, "fake gem").unwrap();

        let result = find_gem_for_ruby(&ruby_path);
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), gem_path);
    }

    #[test]
    fn test_parse_version_from_asset() {
        let suffix = ".x86_64_linux.tar.gz";

        // Standard version
        assert_eq!(
            parse_version_from_asset("ruby-3.4.8.x86_64_linux.tar.gz", suffix),
            Some(semver::Version::new(3, 4, 8))
        );

        // Different version
        assert_eq!(
            parse_version_from_asset("ruby-3.3.0.x86_64_linux.tar.gz", suffix),
            Some(semver::Version::new(3, 3, 0))
        );

        // Wrong platform: should not match
        assert_eq!(
            parse_version_from_asset("ruby-3.4.8.arm64_linux.tar.gz", suffix),
            None
        );

        // Pre-release: filtered out
        assert_eq!(
            parse_version_from_asset("ruby-3.5.0-preview1.x86_64_linux.tar.gz", suffix),
            None
        );

        // Non-semver (two components): filtered out
        assert_eq!(
            parse_version_from_asset("ruby-0.49.x86_64_linux.tar.gz", suffix),
            None
        );

        // Not a ruby asset
        assert_eq!(
            parse_version_from_asset("something-else.tar.gz", suffix),
            None
        );
    }

    #[test]
    fn test_rv_platform_string_for_macos() {
        let intel = Triple::from_str("x86_64-apple-darwin").unwrap();
        assert_eq!(rv_platform_string(&intel), Some("ventura"));

        let arm = Triple::from_str("aarch64-apple-darwin").unwrap();
        assert_eq!(rv_platform_string(&arm), Some("arm64_sonoma"));
    }

    #[test]
    fn test_rv_platform_string_for_linux() {
        let gnu = Triple::from_str("x86_64-unknown-linux-gnu").unwrap();
        assert_eq!(rv_platform_string(&gnu), Some("x86_64_linux"));

        let arm_gnu = Triple::from_str("aarch64-unknown-linux-gnu").unwrap();
        assert_eq!(rv_platform_string(&arm_gnu), Some("arm64_linux"));

        let musl = Triple::from_str("x86_64-unknown-linux-musl").unwrap();
        assert_eq!(rv_platform_string(&musl), Some("x86_64_linux_musl"));

        let arm_musl = Triple::from_str("aarch64-unknown-linux-musl").unwrap();
        assert_eq!(rv_platform_string(&arm_musl,), Some("arm64_linux_musl"));
    }

    #[test]
    fn test_rv_platform_string_unsupported() {
        let windows = Triple::from_str("x86_64-pc-windows-msvc").unwrap();
        assert_eq!(rv_platform_string(&windows), None);

        let linux_unknown_libc = Triple::from_str("x86_64-unknown-linux-gnux32").unwrap();
        assert_eq!(rv_platform_string(&linux_unknown_libc), None);
    }

    #[test]
    fn test_find_installed_empty_dir() {
        let temp_dir = TempDir::new().unwrap();
        let installer = RubyInstaller::new(temp_dir.path().to_path_buf());

        assert!(installer.find_installed(&RubyRequest::Any).is_none());
    }

    #[test]
    fn test_find_installed_with_versions() {
        let temp_dir = TempDir::new().unwrap();

        // Create fake Ruby installations
        for version in ["3.3.5", "3.4.8", "3.2.1"] {
            let bin_dir = temp_dir.path().join(version).join("bin");
            fs::create_dir_all(&bin_dir).unwrap();
            fs::write(bin_dir.join("ruby").with_extension(EXE_EXTENSION), "fake").unwrap();
            fs::write(bin_dir.join(test_gem_executable()), "fake").unwrap();
        }

        let installer = RubyInstaller::new(temp_dir.path().to_path_buf());

        // Any: should return highest version
        let result = installer.find_installed(&RubyRequest::Any).unwrap();
        assert_eq!(*result.version(), semver::Version::new(3, 4, 8));
        assert_eq!(
            result.gem_bin(),
            temp_dir
                .path()
                .join("3.4.8")
                .join("bin")
                .join(test_gem_executable())
        );

        // MajorMinor(3, 3): should return 3.3.5
        let result = installer
            .find_installed(&RubyRequest::MajorMinor(3, 3))
            .unwrap();
        assert_eq!(*result.version(), semver::Version::new(3, 3, 5));

        // Exact match
        let result = installer
            .find_installed(&RubyRequest::Exact(3, 2, 1))
            .unwrap();
        assert_eq!(*result.version(), semver::Version::new(3, 2, 1));

        // No match
        assert!(
            installer
                .find_installed(&RubyRequest::MajorMinor(2, 7))
                .is_none()
        );
    }

    #[test]
    fn test_github_repo_path() {
        assert_eq!(
            github_repo_path("https://github.com/spinel-coop/rv-ruby").as_deref(),
            Some("/spinel-coop/rv-ruby")
        );
        assert_eq!(
            github_repo_path("https://github.com:443/org/repo").as_deref(),
            Some("/org/repo")
        );

        assert!(github_repo_path("http://github.com/org/repo").is_none());
        assert!(github_repo_path("https://gitlab.com/org/repo").is_none());
        assert!(github_repo_path("https://my-mirror.example.com/rv-ruby").is_none());
        assert!(github_repo_path("https://evil.com/github.com/rv").is_none());
        assert!(github_repo_path("https://api.github.com/repos/org/repo").is_none());
        assert!(github_repo_path("https://github.com@evil.com/org/repo").is_none());
        assert!(github_repo_path("https://github.com:password@evil.com/org/repo").is_none());
        assert!(github_repo_path("https://evil.com@github.com/org/repo").is_none());
        assert!(github_repo_path("https://github.com:444/org/repo").is_none());
        assert!(github_repo_path("https://github.com/org").is_none());
        assert!(github_repo_path("https://github.com/org/repo/releases").is_none());
        assert!(github_repo_path("ftp://github.com/org/repo").is_none());
    }

    #[test]
    fn rv_ruby_env_overrides() {
        assert_eq!(
            rv_ruby_base_url(&EnvVars::from_map(&[])),
            RV_RUBY_DEFAULT_URL
        );
        assert_eq!(
            rv_ruby_base_url(&EnvVars::from_map(&[(
                EnvVars::PREK_RUBY_MIRROR,
                "https://example.com/rv-ruby/",
            )])),
            "https://example.com/rv-ruby"
        );

        assert_eq!(
            rv_ruby_api_url(&EnvVars::from_map(&[])),
            (
                "https://api.github.com/repos/spinel-coop/rv-ruby/releases/latest".to_string(),
                true,
            )
        );
        assert_eq!(
            rv_ruby_api_url(&EnvVars::from_map(&[(
                EnvVars::PREK_RUBY_MIRROR,
                "https://mirror.example.com/rv-ruby",
            )])),
            (
                "https://mirror.example.com/rv-ruby/releases/latest".to_string(),
                false,
            )
        );

        let env_vars = EnvVars::from_map(&[(EnvVars::GITHUB_TOKEN, "secret-token")]);
        let client = reqwest::Client::new();

        let request = maybe_add_github_auth(client.get("https://example.com"), true, &env_vars)
            .build()
            .unwrap();
        assert_eq!(
            request
                .headers()
                .get(reqwest::header::AUTHORIZATION)
                .unwrap(),
            "Bearer secret-token"
        );

        let request = maybe_add_github_auth(client.get("https://example.com"), false, &env_vars)
            .build()
            .unwrap();
        assert!(
            !request
                .headers()
                .contains_key(reqwest::header::AUTHORIZATION)
        );
    }

    #[test]
    fn test_find_installed_skips_incomplete_dirs() {
        let temp_dir = TempDir::new().unwrap();

        // Version dir with ruby but no gem
        let bin_dir = temp_dir.path().join("3.4.8").join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("ruby").with_extension(EXE_EXTENSION), "fake").unwrap();

        // Version dir with no bin at all
        fs::create_dir_all(temp_dir.path().join("3.3.0")).unwrap();

        // Non-version directory
        fs::create_dir_all(temp_dir.path().join("not-a-version").join("bin")).unwrap();

        let installer = RubyInstaller::new(temp_dir.path().to_path_buf());
        assert!(installer.find_installed(&RubyRequest::Any).is_none());
    }
}
