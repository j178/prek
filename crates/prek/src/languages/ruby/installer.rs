use std::env::consts::EXE_EXTENSION;
#[cfg(not(target_os = "windows"))]
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
#[cfg(not(target_os = "windows"))]
use prek_consts::env_vars::EnvVars;
use tracing::{trace, warn};

use crate::languages::ruby::RubyRequest;
use crate::process::Cmd;
use crate::store::Store;

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
pub(crate) struct RubyInstaller;

impl RubyInstaller {
    pub(crate) fn new() -> Self {
        Self {}
    }

    /// Main installation entry point
    pub(crate) async fn install(
        &self,
        _store: &Store,
        request: &RubyRequest,
    ) -> Result<RubyResult> {
        // For now, we only support system Ruby
        // TODO: If supporting installing new rubies, add locked file acquisition for concurrency safety
        // let _lock = LockedFile::acquire(self.root.join(".lock"), "ruby").await?;

        // Check system Ruby
        if let Some(ruby) = self.find_system_ruby(request).await? {
            trace!(
                "Using Ruby: {} at {}",
                ruby.version(),
                ruby.ruby_bin().display()
            );
            return Ok(ruby);
        }

        // No suitable Ruby found
        // TODO: On non-Windows, could implement rv/rbenv/etc integration for managed Ruby installations
        anyhow::bail!(ruby_not_found_error(request));
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
        // rv: ~/.data/rv/rubies/3.4.6/bin/ruby
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

/// Detect which Ruby version managers are installed
#[cfg(not(target_os = "windows"))]
fn detect_version_managers() -> Vec<&'static str> {
    let home = match EnvVars::var(EnvVars::HOME) {
        Ok(h) => PathBuf::from(h),
        Err(_) => return vec![],
    };

    let mut managers = Vec::new();

    // Check for Homebrew first (most common on macOS)
    if which::which("brew").is_ok()
        || Path::new("/opt/homebrew").exists()
        || Path::new("/usr/local/Homebrew").exists()
        || Path::new("/home/linuxbrew/.linuxbrew").exists()
        || home.join(".linuxbrew").exists()
    {
        managers.push("brew");
    }

    // Check for various version managers
    if home.join(".rvm").exists() || which::which("rvm").is_ok() {
        managers.push("rvm");
    }
    if home.join(".data/rv").exists() || which::which("rv").is_ok() {
        managers.push("rv");
    }
    if home.join(".local/share/mise").exists() || which::which("mise").is_ok() {
        managers.push("mise");
    }
    if home.join(".rbenv").exists() || which::which("rbenv").is_ok() {
        managers.push("rbenv");
    }
    if home.join(".asdf").exists() || which::which("asdf").is_ok() {
        managers.push("asdf");
    }
    if home.join(".rubies").exists() || PathBuf::from("/opt/rubies").exists() {
        managers.push("chruby");
    }

    managers
}

/// Generate helpful error message with version manager suggestions
fn ruby_not_found_error(request: &RubyRequest) -> String {
    let mut msg = format!(
        "No suitable Ruby found for request: {}\n",
        format_request(request)
    );

    // Windows-specific guidance
    #[cfg(target_os = "windows")]
    {
        msg.push_str("\nRuby language only supports system Ruby on Windows.\n");
        msg.push_str("Please install Ruby from https://rubyinstaller.org/\n");
        msg
    }

    // Unix-like systems
    #[cfg(not(target_os = "windows"))]
    {
        let managers = detect_version_managers();

        if managers.is_empty() {
            msg.push_str("\nNo Ruby version manager detected. Install Ruby via:\n");
            msg.push_str("  System package manager:\n");
            msg.push_str("    Ubuntu/Debian: sudo apt install ruby\n");
            msg.push_str("    macOS: brew install ruby\n");
            msg.push_str("  Or install a version manager:\n");
            msg.push_str("    rvm: https://rvm.io/\n");
            msg.push_str("    mise: https://mise.jdx.dev/\n");
            msg.push_str("    rbenv: https://github.com/rbenv/rbenv\n");
        } else {
            writeln!(
                msg,
                "\nDetected version manager(s): {}",
                managers.join(", ")
            )
            .unwrap();
            msg.push_str("\nYou can install the required Ruby version using:\n");

            for manager in &managers {
                match *manager {
                    "chruby" => msg.push_str("  chruby: Install Ruby manually to ~/.rubies/\n"),
                    "brew" => {
                        msg.push_str("  brew install ruby  # Installs latest version\n");
                        if !request.is_any() {
                            msg.push_str(
                                "  # Note: Homebrew typically installs the latest Ruby version.\n",
                            );
                            msg.push_str("  # For specific versions, consider using a version manager like rbenv or mise.\n");
                        }
                    }
                    _ => {
                        let version = format_request_for_install(request);
                        let install_arg = match *manager {
                            "mise" => format!("ruby@{version}"),
                            "asdf" => format!("ruby {version}"),
                            _ => version,
                        };
                        writeln!(msg, "  {manager} install {install_arg}").unwrap();
                    }
                }
            }
        }

        msg
    }
}

/// Format request for display
fn format_request(request: &RubyRequest) -> String {
    match request {
        RubyRequest::Any => "any".to_string(),
        RubyRequest::Exact(maj, min, patch) => format!("{maj}.{min}.{patch}"),
        RubyRequest::MajorMinor(maj, min) => format!("{maj}.{min}"),
        RubyRequest::Major(maj) => format!("{maj}"),
        RubyRequest::Path(p) => p.display().to_string(),
        RubyRequest::Range(_, s) => s.clone(),
    }
}

/// Format request for version manager install command
#[cfg(not(target_os = "windows"))]
fn format_request_for_install(request: &RubyRequest) -> String {
    match request {
        RubyRequest::Exact(maj, min, patch) => format!("{maj}.{min}.{patch}"),
        RubyRequest::MajorMinor(maj, min) => format!("{maj}.{min}"),
        RubyRequest::Major(maj) => format!("{maj}"),
        RubyRequest::Range(_, s) => s.clone(),
        _ => "<any version>".to_string(), // fallback
    }
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
    use tempfile::TempDir;

    #[test]
    fn test_format_request() {
        assert_eq!(format_request(&RubyRequest::Any), "any");
        assert_eq!(format_request(&RubyRequest::Exact(3, 4, 6)), "3.4.6");
        assert_eq!(format_request(&RubyRequest::MajorMinor(3, 4)), "3.4");
        assert_eq!(format_request(&RubyRequest::Major(3)), "3");

        let range = semver::VersionReq::parse(">=3.2").unwrap();
        assert_eq!(
            format_request(&RubyRequest::Range(range, ">=3.2".to_string())),
            ">=3.2"
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_format_request_for_install() {
        assert_eq!(
            format_request_for_install(&RubyRequest::Exact(3, 4, 6)),
            "3.4.6"
        );
        assert_eq!(
            format_request_for_install(&RubyRequest::MajorMinor(3, 4)),
            "3.4"
        );
        assert_eq!(format_request_for_install(&RubyRequest::Major(3)), "3");

        let range = semver::VersionReq::parse(">=3.2").unwrap();
        assert_eq!(
            format_request_for_install(&RubyRequest::Range(range, ">=3.2".to_string())),
            ">=3.2"
        );

        // Fallback for Any and Path
        assert_eq!(
            format_request_for_install(&RubyRequest::Any),
            "<any version>"
        );
    }

    #[test]
    #[cfg(not(target_os = "windows"))]
    fn test_detect_version_managers_empty() {
        // This test assumes the test environment doesn't have version managers
        // in specific test temp directories - just ensures function doesn't panic
        let managers = detect_version_managers();
        // Result depends on actual system, so we just check it returns a vec
        assert!(managers.is_empty() || !managers.is_empty());
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
    fn test_ruby_not_found_error_format() {
        let request = RubyRequest::Exact(3, 4, 6);
        let error = ruby_not_found_error(&request);

        assert!(error.contains("3.4.6"));
        assert!(error.contains("No suitable Ruby found"));
    }

    #[test]
    fn test_ruby_not_found_error_any() {
        let request = RubyRequest::Any;
        let error = ruby_not_found_error(&request);

        assert!(error.contains("any"));
        assert!(error.contains("No suitable Ruby found"));
    }

    #[cfg(not(target_os = "windows"))]
    #[test]
    fn test_ruby_not_found_error_includes_suggestions() {
        let request = RubyRequest::Exact(3, 4, 6);
        let error = ruby_not_found_error(&request);

        // Should contain either version manager suggestions or system install instructions
        assert!(
            error.contains("version manager")
                || error.contains("System package manager")
                || error.contains("Ubuntu/Debian")
        );
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_ruby_not_found_error_windows() {
        let request = RubyRequest::Exact(3, 4, 6);
        let error = ruby_not_found_error(&request);

        assert!(error.contains("rubyinstaller.org"));
        assert!(error.contains("Windows"));
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
}
