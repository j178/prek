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
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Swift;

pub(crate) struct SwiftInfo {
    pub(crate) version: Version,
    pub(crate) executable: PathBuf,
}

pub(crate) async fn query_swift_info() -> Result<SwiftInfo> {
    // Find swift executable
    let executable = which::which("swift").context("Swift not found on PATH")?;

    // macOS: "swift-driver version: X.Y.Z Apple Swift version X.Y.Z ..."
    // Linux/Windows: "Swift version X.Y.Z ..."
    let stdout = Cmd::new("swift", "get swift version")
        .arg("--version")
        .check(true)
        .output()
        .await?
        .stdout;

    let output = String::from_utf8_lossy(&stdout);
    let version = parse_swift_version(&output).context("Failed to parse Swift version")?;

    Ok(SwiftInfo {
        version,
        executable,
    })
}

/// Normalize version string to semver format (e.g., "5.10" -> "5.10.0").
/// Some Swift toolchains report versions without a patch component.
fn normalize_version(version_str: &str) -> String {
    // Strip any pre-release suffix (e.g., "6.0-dev" -> "6.0")
    let version_str = version_str.split('-').next().unwrap_or(version_str);
    if version_str.matches('.').count() == 1 {
        format!("{version_str}.0")
    } else {
        version_str.to_string()
    }
}

fn parse_swift_version(output: &str) -> Option<Version> {
    for line in output.lines() {
        // Try Apple Swift format (macOS) - may appear mid-line
        if let Some(idx) = line.find("Apple Swift version ") {
            let rest = &line[idx + "Apple Swift version ".len()..];
            if let Some(version_str) = rest.split_whitespace().next() {
                if let Ok(version) = normalize_version(version_str).parse() {
                    return Some(version);
                }
            }
        }
        // Try plain Swift format (Linux) - at start of line
        if let Some(rest) = line.strip_prefix("Swift version ") {
            let version_str = rest.split_whitespace().next()?;
            return normalize_version(version_str).parse().ok();
        }
    }
    None
}

fn build_dir(env_path: &Path) -> PathBuf {
    env_path.join(".build")
}

const BIN_PATH_KEY: &str = "swift_bin_path";

impl LanguageImpl for Swift {
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

        debug!(%hook, target = %info.env_path.display(), "Installing Swift environment");

        // Query swift info
        let swift_info = query_swift_info()
            .await
            .context("Failed to query Swift info")?;

        // Build if repo has Package.swift
        if let Some(repo_path) = hook.repo_path() {
            if repo_path.join("Package.swift").exists() {
                debug!(%hook, "Building Swift package");
                let build_path = build_dir(&info.env_path);
                Cmd::new("swift", "swift build")
                    .arg("build")
                    .arg("-c")
                    .arg("release")
                    .arg("--package-path")
                    .arg(repo_path)
                    .arg("--build-path")
                    .arg(&build_path)
                    .check(true)
                    .output()
                    .await
                    .context("Failed to build Swift package")?;

                // Get the actual bin path (includes target triple, e.g., .build/arm64-apple-macosx/release)
                let bin_path_output = Cmd::new("swift", "get bin path")
                    .arg("build")
                    .arg("-c")
                    .arg("release")
                    .arg("--package-path")
                    .arg(repo_path)
                    .arg("--build-path")
                    .arg(&build_path)
                    .arg("--show-bin-path")
                    .check(true)
                    .output()
                    .await
                    .context("Failed to get Swift bin path")?;
                let bin_path = String::from_utf8_lossy(&bin_path_output.stdout)
                    .trim()
                    .to_string();
                debug!(%hook, %bin_path, "Swift bin path");
                info.with_extra(BIN_PATH_KEY, &bin_path);
            } else {
                debug!(%hook, "No Package.swift found, skipping build");
            }
        }

        info.with_toolchain(swift_info.executable)
            .with_language_version(swift_info.version);

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        // Verify swift still exists at the stored path
        if !info.toolchain.exists() {
            anyhow::bail!(
                "Swift executable no longer exists at: {}",
                info.toolchain.display()
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

        // Get bin path from install info if a package was built
        let new_path =
            if let Some(bin_path) = hook.install_info().and_then(|i| i.get_extra(BIN_PATH_KEY)) {
                prepend_paths(&[Path::new(bin_path)]).context("Failed to join PATH")?
            } else {
                EnvVars::var_os(EnvVars::PATH).unwrap_or_default()
            };

        let entry = hook.entry.resolve(Some(&new_path))?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "swift hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
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

#[cfg(test)]
mod tests {
    use super::parse_swift_version;

    #[test]
    fn test_parse_macos_format() {
        // macOS: "swift-driver version: ... Apple Swift version X.Y.Z ..."
        let output = "swift-driver version: 1.115.0 Apple Swift version 6.1.2 (swiftlang-6.1.2.1.1 clang-1700.0.13.1)";
        let version = parse_swift_version(output).unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 1);
        assert_eq!(version.patch, 2);
    }

    #[test]
    fn test_parse_linux_format() {
        // Linux/Windows: "Swift version X.Y.Z ..."
        let output = "Swift version 6.1.2 (swift-6.1.2-RELEASE)";
        let version = parse_swift_version(output).unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 1);
        assert_eq!(version.patch, 2);
    }

    #[test]
    fn test_parse_multiline_output() {
        // macOS output includes target on second line
        let output = r"swift-driver version: 1.115.0 Apple Swift version 6.1.2 (swiftlang-6.1.2.1.1 clang-1700.0.13.1)
Target: arm64-apple-macosx15.0";
        let version = parse_swift_version(output).unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 1);
        assert_eq!(version.patch, 2);
    }

    #[test]
    fn test_parse_linux_multiline() {
        // Linux output includes target on second line
        let output = r"Swift version 6.1.2 (swift-6.1.2-RELEASE)
Target: x86_64-unknown-linux-gnu";
        let version = parse_swift_version(output).unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 1);
        assert_eq!(version.patch, 2);
    }

    #[test]
    fn test_parse_invalid_output() {
        assert!(parse_swift_version("").is_none());
        assert!(parse_swift_version("not a version string").is_none());
        assert!(parse_swift_version("version 6.1.2").is_none()); // Missing "Swift"
    }

    #[test]
    fn test_parse_version_without_patch() {
        // Some toolchains report versions without a patch number
        let output = "swift-driver version: 1.115.0 Apple Swift version 6.1 (swiftlang-6.1.0.0.1 clang-1700.0.13.1)";
        let version = parse_swift_version(output).unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 1);
        assert_eq!(version.patch, 0); // Normalized to .0

        // Linux format without patch
        let output = "Swift version 6.1 (swift-6.1-RELEASE)";
        let version = parse_swift_version(output).unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 1);
        assert_eq!(version.patch, 0);
    }

    #[test]
    fn test_parse_dev_version() {
        // Development/nightly versions have -dev suffix
        let output = "Swift version 6.2-dev (LLVM abcdef, Swift 123456)";
        let version = parse_swift_version(output).unwrap();
        assert_eq!(version.major, 6);
        assert_eq!(version.minor, 2);
        assert_eq!(version.patch, 0);
    }
}
