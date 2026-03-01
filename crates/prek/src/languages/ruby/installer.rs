use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;
use serde::Deserialize;
use target_lexicon::{Architecture, Environment, HOST, OperatingSystem, Triple};
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::languages::ruby::RubyRequest;
use crate::languages::{REQWEST_CLIENT, download_and_extract_with};
use crate::process::Cmd;
use crate::store::Store;

const RV_RUBY_DEFAULT_URL: &str = "https://github.com/spinel-coop/rv-ruby";

/// Resolve the rv-ruby mirror base URL and whether it targets github.com.
fn rv_ruby_mirror() -> (String, bool) {
    match EnvVars::var(EnvVars::PREK_RUBY_MIRROR) {
        Ok(mirror) => {
            let is_github = is_github_https(&mirror);
            (mirror, is_github)
        }
        Err(_) => (RV_RUBY_DEFAULT_URL.to_string(), true),
    }
}

/// Returns a URL compatible with the GitHub Releases API for listing rv-ruby
/// versions, and whether the target host is github.com (for auth token
/// decisions).
///
/// When the mirror is a `github.com` URL, the path is rewritten to use the
/// `api.github.com` host (e.g. `https://github.com/org/repo` becomes
/// `https://api.github.com/repos/org/repo/releases/latest`).
fn rv_ruby_api_url() -> (String, bool) {
    let (base, is_github) = rv_ruby_mirror();
    let url = if is_github {
        // Rewrite github.com web URL to API URL.
        let path = base
            .strip_prefix("https://github.com")
            .expect("is_github_https should ensure this");
        format!("https://api.github.com/repos{path}/releases/latest")
    } else {
        format!("{base}/releases/latest")
    };
    (url, is_github)
}

/// Check whether a URL is an HTTPS URL pointing to github.com.
/// Only matches the exact host `github.com` over HTTPS, so won't send
/// tokens to other hosts, subdomains, path-injection attempts,
/// userinfo-based redirects, or plaintext HTTP.
fn is_github_https(url: &str) -> bool {
    (url.starts_with("https://github.com/") || url.starts_with("https://github.com:"))
        && !url.contains('@')
}

/// Returns the base URL for downloading rv-ruby release assets, and whether
/// the target host is github.com (for auth token decisions).
fn rv_ruby_download_base() -> (String, bool) {
    let (base, is_github) = rv_ruby_mirror();
    (format!("{base}/releases/latest/download"), is_github)
}

/// Conditionally add a GitHub auth token to a request builder.
/// Only sends `GITHUB_TOKEN` when `is_github` is true.
fn maybe_add_github_auth(req: reqwest::RequestBuilder, is_github: bool) -> reqwest::RequestBuilder {
    if is_github {
        if let Ok(token) = EnvVars::var(EnvVars::GITHUB_TOKEN) {
            return req.header(http::header::AUTHORIZATION, format!("Bearer {token}"));
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

    /// Ruby version
    version: semver::Version,

    /// Ruby engine (ruby, jruby, truffleruby)
    engine: String,
}

impl RubyResult {
    pub(crate) fn ruby_bin(&self) -> &Path {
        &self.ruby_bin
    }

    pub(crate) fn version(&self) -> &semver::Version {
        &self.version
    }

    pub(crate) fn engine(&self) -> &str {
        &self.engine
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

        let Some(version) = versions.into_iter().find(|v| request.matches(v, None)) else {
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
                let bin_dir = entry.path().join("bin");
                let ruby_bin = bin_dir.join("ruby");
                let gem_bin = bin_dir.join("gem");
                if ruby_bin.exists() && gem_bin.exists() {
                    Some((version, ruby_bin))
                } else {
                    None
                }
            })
            .sorted_unstable_by(|(a, _), (b, _)| b.cmp(a)) // descending
            .find_map(|(version, ruby_bin)| {
                if request.matches(&version, Some(&ruby_bin)) {
                    Some(RubyResult {
                        ruby_bin,
                        version,
                        engine: "ruby".to_string(),
                    })
                } else {
                    None
                }
            })
    }

    /// Fetch available Ruby versions from the rv-ruby GitHub release.
    async fn list_remote_versions(&self, platform: &str) -> Result<Vec<semver::Version>> {
        let (api_url, is_github) = rv_ruby_api_url();
        let suffix = format!(".{platform}.tar.gz");

        let req = REQWEST_CLIENT
            .get(&api_url)
            .header("Accept", "application/vnd.github+json");
        let req = maybe_add_github_auth(req, is_github);

        let response = req
            .send()
            .await
            .with_context(|| format!("Failed to fetch rv-ruby releases from {api_url}"))?;

        if !response.status().is_success() {
            let status = response.status();
            let hint = if matches!(status.as_u16(), 403 | 429) {
                " (this may be a rate limit — try setting GITHUB_TOKEN)"
            } else {
                ""
            };
            anyhow::bail!("Failed to fetch rv-ruby releases from {api_url}: {status}{hint}");
        }

        let release: GitHubRelease = response
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
    /// Uses `download_and_extract_with` to inject a `GITHUB_TOKEN` auth header
    /// for GitHub-hosted mirrors (including private partial mirrors of rv-ruby).
    async fn download(
        &self,
        store: &Store,
        version: &semver::Version,
        platform: &str,
    ) -> Result<RubyResult> {
        let filename = format!("ruby-{version}.{platform}.tar.gz");
        let (base_url, is_github) = rv_ruby_download_base();
        let url = format!("{base_url}/{filename}");
        let version_str = version.to_string();
        let target = self.root.join(&version_str);

        debug!(url = %url, target = %target.display(), "Downloading Ruby {version}");

        download_and_extract_with(
            &url,
            &filename,
            store,
            |req| maybe_add_github_auth(req, is_github),
            async |extracted| {
                // rv-ruby tarballs contain: rv-ruby@{version}/{version}/bin/ruby
                // After strip_component, `extracted` is the rv-ruby@{version}/ directory.
                // Move the inner {version}/ directory to our target.
                let inner = extracted.join(&version_str);
                if !inner.exists() {
                    anyhow::bail!(
                        "Expected directory '{}' inside rv-ruby archive, found: {:?}",
                        version_str,
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
                Ok(())
            },
        )
        .await
        .with_context(|| format!("Failed to download Ruby {version} from {url}"))?;

        Ok(RubyResult {
            ruby_bin: target.join("bin").join("ruby"),
            version: version.clone(),
            engine: "ruby".to_string(),
        })
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
    // Check for gem in same directory
    if let Err(e) = find_gem_for_ruby(ruby_path) {
        warn!("Ruby at {} has no gem: {}", ruby_path.display(), e);
        return None;
    }

    // Query version and engine
    match query_ruby_info(ruby_path).await {
        Ok((version, engine)) => {
            let result = RubyResult {
                ruby_bin: ruby_path.to_path_buf(),
                version,
                engine,
            };

            if request.matches(&result.version, Some(&result.ruby_bin)) {
                Some(result)
            } else {
                None
            }
        }
        Err(e) => {
            warn!("Failed to query Ruby at {}: {}", ruby_path.display(), e);
            None
        }
    }
}

/// Search version manager directories for suitable Ruby installations
#[cfg(not(target_os = "windows"))]
async fn search_version_managers(request: &RubyRequest) -> Option<RubyResult> {
    let home = EnvVars::var(EnvVars::HOME).ok()?;
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
    let entries = std::fs::read_dir(dir).ok()?;

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
    let ruby_dir = ruby_path
        .parent()
        .context("Ruby executable has no parent directory")?;

    // Try various gem executable names (for Windows compatibility)
    for name in ["gem", "gem.bat", "gem.cmd"] {
        let gem_path = ruby_dir.join(name).with_extension(EXE_EXTENSION);
        if gem_path.exists() {
            return Ok(gem_path);
        }

        // Also try without explicit extension
        let gem_path = ruby_dir.join(name);
        if gem_path.exists() {
            return Ok(gem_path);
        }
    }

    anyhow::bail!(
        "No gem executable found alongside Ruby at {}",
        ruby_path.display()
    )
}

/// Query Ruby version and engine
async fn query_ruby_info(ruby_path: &Path) -> Result<(semver::Version, String)> {
    let script = "puts RUBY_ENGINE; puts RUBY_VERSION";

    let output = Cmd::new(ruby_path, "query ruby version")
        .arg("-e")
        .arg(script)
        .check(true)
        .output()
        .await?;

    let mut lines = str::from_utf8(&output.stdout)?.lines();
    let engine = lines.next().unwrap_or("ruby").to_string();
    let version_str = lines.next().context("No version in Ruby output")?.trim();

    let version = semver::Version::parse(version_str)
        .with_context(|| format!("Failed to parse Ruby version: {version_str}"))?;

    Ok((version, engine))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::str::FromStr;
    use target_lexicon::Triple;
    use tempfile::TempDir;

    /// Mutex to serialize tests that mutate `PREK_RUBY_MIRROR`.
    static ENV_MUTEX: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that serializes env var access and restores the original value on drop.
    /// Holds the `ENV_MUTEX` lock for its lifetime, so tests using this guard run
    /// sequentially. Ensures cleanup even if a test panics.
    struct EnvVarGuard {
        key: &'static str,
        saved: Option<String>,
        _lock: std::sync::MutexGuard<'static, ()>,
    }

    impl EnvVarGuard {
        fn new(key: &'static str) -> Self {
            let lock = ENV_MUTEX
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let saved = EnvVars::var(key).ok();
            Self {
                key,
                saved,
                _lock: lock,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.saved {
                Some(v) => unsafe { std::env::set_var(self.key, v) },
                None => unsafe { std::env::remove_var(self.key) },
            }
        }
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
    fn test_rv_ruby_urls_default() {
        let _guard = EnvVarGuard::new(EnvVars::PREK_RUBY_MIRROR);
        unsafe { std::env::remove_var(EnvVars::PREK_RUBY_MIRROR) };

        let (api_url, api_is_github) = rv_ruby_api_url();
        assert_eq!(
            api_url,
            "https://api.github.com/repos/spinel-coop/rv-ruby/releases/latest"
        );
        assert!(api_is_github);

        let (dl_url, dl_is_github) = rv_ruby_download_base();
        assert_eq!(
            dl_url,
            format!("{RV_RUBY_DEFAULT_URL}/releases/latest/download")
        );
        assert!(dl_is_github);
    }

    #[test]
    fn test_rv_ruby_urls_github_mirror() {
        // A github.com mirror: API URL is rewritten, download URL uses web URL.
        let _guard = EnvVarGuard::new(EnvVars::PREK_RUBY_MIRROR);
        unsafe {
            std::env::set_var(
                EnvVars::PREK_RUBY_MIRROR,
                "https://github.com/myorg/vetted-rubies",
            );
        }

        let (api_url, api_is_github) = rv_ruby_api_url();
        assert_eq!(
            api_url,
            "https://api.github.com/repos/myorg/vetted-rubies/releases/latest"
        );
        assert!(api_is_github);

        let (dl_url, dl_is_github) = rv_ruby_download_base();
        assert_eq!(
            dl_url,
            "https://github.com/myorg/vetted-rubies/releases/latest/download"
        );
        assert!(dl_is_github);
    }

    #[test]
    fn test_rv_ruby_urls_non_github_mirror() {
        // A non-github mirror: both URLs use the mirror as-is, is_github is false.
        let _guard = EnvVarGuard::new(EnvVars::PREK_RUBY_MIRROR);
        unsafe {
            std::env::set_var(
                EnvVars::PREK_RUBY_MIRROR,
                "https://my-mirror.example.com/rv-ruby",
            );
        }

        let (api_url, api_is_github) = rv_ruby_api_url();
        assert_eq!(
            api_url,
            "https://my-mirror.example.com/rv-ruby/releases/latest"
        );
        assert!(!api_is_github);

        let (dl_url, dl_is_github) = rv_ruby_download_base();
        assert_eq!(
            dl_url,
            "https://my-mirror.example.com/rv-ruby/releases/latest/download"
        );
        assert!(!dl_is_github);
    }

    #[test]
    fn test_find_gem_for_ruby_missing() {
        let temp_dir = TempDir::new().unwrap();
        let ruby_path = temp_dir.path().join("bin/ruby");

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

        let ruby_path = bin_dir.join("ruby");
        let gem_path = bin_dir.join("gem");

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
            fs::write(bin_dir.join("ruby"), "fake").unwrap();
            fs::write(bin_dir.join("gem"), "fake").unwrap();
        }

        let installer = RubyInstaller::new(temp_dir.path().to_path_buf());

        // Any: should return highest version
        let result = installer.find_installed(&RubyRequest::Any).unwrap();
        assert_eq!(*result.version(), semver::Version::new(3, 4, 8));

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
    fn test_is_github_https() {
        // Exact match over HTTPS
        assert!(is_github_https("https://github.com/spinel-coop/rv-ruby"));
        assert!(is_github_https("https://github.com:443/org/repo"));

        // Plaintext HTTP — don't leak tokens
        assert!(!is_github_https("http://github.com/org/repo"));
        // Not github.com
        assert!(!is_github_https("https://gitlab.com/org/repo"));
        assert!(!is_github_https("https://my-mirror.example.com/rv-ruby"));
        // Path injection — github.com in path, not host
        assert!(!is_github_https("https://evil.com/github.com/rv"));
        // Subdomain — not the same host
        assert!(!is_github_https("https://api.github.com/repos/org/repo"));
        // Userinfo-based redirect
        assert!(!is_github_https("https://github.com@evil.com/org/repo"));
        assert!(!is_github_https(
            "https://github.com:password@evil.com/org/repo"
        ));
        assert!(!is_github_https("https://evil.com@github.com/org/repo"));
        // Other schemes
        assert!(!is_github_https("ftp://github.com/org/repo"));
    }

    #[test]
    fn test_find_installed_skips_incomplete_dirs() {
        let temp_dir = TempDir::new().unwrap();

        // Version dir with ruby but no gem
        let bin_dir = temp_dir.path().join("3.4.8").join("bin");
        fs::create_dir_all(&bin_dir).unwrap();
        fs::write(bin_dir.join("ruby"), "fake").unwrap();

        // Version dir with no bin at all
        fs::create_dir_all(temp_dir.path().join("3.3.0")).unwrap();

        // Non-version directory
        fs::create_dir_all(temp_dir.path().join("not-a-version").join("bin")).unwrap();

        let installer = RubyInstaller::new(temp_dir.path().to_path_buf());
        assert!(installer.find_installed(&RubyRequest::Any).is_none());
    }
}
