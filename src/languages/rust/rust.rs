use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context;
use itertools::{Either, Itertools};
use prek_consts::env_vars::EnvVars;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::rust::RustRequest;
use crate::languages::rust::installer::{RustInstaller, rustup_home_dir};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::{Store, ToolBucket};

fn format_cargo_dependency(dep: &str) -> String {
    let (name, version) = dep.split_once(':').unwrap_or((dep, ""));
    if version.is_empty() {
        format!("{name}@*")
    } else {
        format!("{name}@{version}")
    }
}

/// Extract package name from Cargo.toml content.
fn extract_package_name(content: &str) -> Option<String> {
    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("name") {
            if let Some((_key, value)) = line.split_once('=') {
                let name = value.trim().trim_matches('"').trim_matches('\'');
                return Some(name.to_string());
            }
        }
    }
    None
}

/// Find the package directory that produces the given binary.
/// Returns (`package_dir`, `is_workspace`).
async fn find_package_dir(repo: &Path, binary_name: &str) -> anyhow::Result<(PathBuf, bool)> {
    let root_cargo = repo.join("Cargo.toml");
    if !root_cargo.exists() {
        anyhow::bail!("No Cargo.toml found in {}", repo.display());
    }

    let content = fs_err::tokio::read_to_string(&root_cargo).await?;

    // If it's a workspace, search member directories AND the root
    if content.contains("[workspace]") {
        // First, check if the root itself is also a package
        if content.contains("[package]") {
            if let Some(name) = extract_package_name(&content) {
                if name == binary_name || name.replace('-', "_") == binary_name.replace('-', "_") {
                    return Ok((repo.to_path_buf(), true));
                }
            }
        }

        // Then search subdirectories
        let mut entries = fs_err::tokio::read_dir(repo).await?;

        while let Some(entry) = entries.next_entry().await? {
            if entry.file_type().await?.is_dir() {
                let member_cargo = entry.path().join("Cargo.toml");
                if member_cargo.exists() {
                    let member_content = fs_err::tokio::read_to_string(&member_cargo).await?;
                    if let Some(name) = extract_package_name(&member_content) {
                        if name == binary_name
                            || name.replace('-', "_") == binary_name.replace('-', "_")
                        {
                            return Ok((entry.path(), true));
                        }
                    }
                }
            }
        }
        anyhow::bail!(
            "No package found for binary '{}' in workspace {}",
            binary_name,
            repo.display()
        );
    }

    // Single package at root
    if content.contains("[package]") {
        return Ok((repo.to_path_buf(), false));
    }

    anyhow::bail!("Invalid Cargo.toml in {}", repo.display());
}

#[derive(Debug, Copy, Clone)]
pub(crate) struct Rust;

impl LanguageImpl for Rust {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> anyhow::Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install Rust
        let rust_dir = store.tools_path(ToolBucket::Rust);
        let installer = RustInstaller::new(rust_dir);

        let (version, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&RustRequest::Any, !system_only),
            LanguageRequest::Rust(version) => (version, true),
            _ => unreachable!(),
        };

        let rust = installer
            .install(store, version, allows_download)
            .await
            .context("Failed to install rust")?;

        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;
        info.with_toolchain(rust.bin().to_path_buf())
            .with_language_version(rust.version().deref().clone());

        // Store the channel name for cache matching
        match version {
            RustRequest::Channel(channel) => {
                info.with_extra("rust_channel", channel);
            }
            RustRequest::Any => {
                // Any resolves to "stable" in resolve_version
                info.with_extra("rust_channel", "stable");
            }
            _ => {}
        }

        // 2. Create environment
        fs_err::tokio::create_dir_all(bin_dir(&info.env_path)).await?;

        // 3. Install dependencies
        let cargo_home = &info.env_path;

        // Split dependencies by cli: prefix
        let (cli_deps, lib_deps): (Vec<_>, Vec<_>) =
            hook.additional_dependencies.iter().partition_map(|dep| {
                if let Some(stripped) = dep.strip_prefix("cli:") {
                    Either::Left(stripped)
                } else {
                    Either::Right(dep)
                }
            });

        // Install library dependencies and local project
        if let Some(repo) = hook.repo_path() {
            // Get the binary name from the hook entry
            let entry_parts = hook.entry.split()?;
            let binary_name = &entry_parts[0];

            // Find the specific package directory for this hook's binary
            let (package_dir, is_workspace) = find_package_dir(repo, binary_name).await?;

            if lib_deps.is_empty() {
                // Install directly
                Cmd::new("cargo", "install local")
                    .args(["install", "--bins", "--root"])
                    .arg(cargo_home)
                    .args(["--path", "."])
                    .current_dir(&package_dir)
                    .env(EnvVars::CARGO_HOME, cargo_home)
                    .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                    .remove_git_env()
                    .check(true)
                    .output()
                    .await?;
            } else {
                // Copy manifest files to env directory, modify there, build with --manifest-path
                let manifest_dir = info.env_path.join("manifest");
                fs_err::tokio::create_dir_all(&manifest_dir).await?;

                // Copy Cargo.toml
                let src_manifest = package_dir.join("Cargo.toml");
                let dst_manifest = manifest_dir.join("Cargo.toml");
                fs_err::tokio::copy(&src_manifest, &dst_manifest).await?;

                // Copy Cargo.lock if it exists (check both package dir and repo root for workspaces)
                let lock_locations = if is_workspace {
                    vec![repo.join("Cargo.lock"), package_dir.join("Cargo.lock")]
                } else {
                    vec![package_dir.join("Cargo.lock")]
                };
                for lock_path in lock_locations {
                    if lock_path.exists() {
                        fs_err::tokio::copy(&lock_path, manifest_dir.join("Cargo.lock")).await?;
                        break;
                    }
                }

                // Copy src directory (cargo add needs it to exist for path validation)
                let src_dir = package_dir.join("src");
                if src_dir.exists() {
                    let dst_src = manifest_dir.join("src");
                    fs_err::tokio::create_dir_all(&dst_src).await?;
                    let mut entries = fs_err::tokio::read_dir(&src_dir).await?;
                    while let Some(entry) = entries.next_entry().await? {
                        if entry.file_type().await?.is_file() {
                            fs_err::tokio::copy(entry.path(), dst_src.join(entry.file_name()))
                                .await?;
                        }
                    }
                }

                // Run cargo add on the copied manifest
                let mut cmd = Cmd::new("cargo", "add dependencies");
                cmd.arg("add");
                for dep in &lib_deps {
                    cmd.arg(format_cargo_dependency(dep.as_str()));
                }
                cmd.current_dir(&manifest_dir)
                    .env(EnvVars::CARGO_HOME, cargo_home)
                    .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                    .remove_git_env()
                    .check(true)
                    .output()
                    .await?;

                // Build using cargo build with --manifest-path pointing to modified manifest
                // but source files come from original package_dir
                let target_dir = info.env_path.join("target");
                Cmd::new("cargo", "build local with deps")
                    .args(["build", "--bins", "--release"])
                    .arg("--manifest-path")
                    .arg(&dst_manifest)
                    .arg("--target-dir")
                    .arg(&target_dir)
                    .current_dir(&package_dir)
                    .env(EnvVars::CARGO_HOME, cargo_home)
                    .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                    .remove_git_env()
                    .check(true)
                    .output()
                    .await?;

                // Copy compiled binaries to the bin directory
                let release_bin_dir = target_dir.join("release");
                let dest_bin_dir = bin_dir(&info.env_path);
                let mut entries = fs_err::tokio::read_dir(&release_bin_dir).await?;
                while let Some(entry) = entries.next_entry().await? {
                    let path = entry.path();
                    let file_type = entry.file_type().await?;
                    // Copy executable files (not directories, not .d files, etc.)
                    if file_type.is_file() {
                        if let Some(ext) = path.extension() {
                            // Skip non-binary files like .d, .rlib, etc.
                            if ext == "d" || ext == "rlib" || ext == "rmeta" {
                                continue;
                            }
                        }
                        // On Unix, check if it's executable; on Windows, check for .exe
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            let meta = entry.metadata().await?;
                            if meta.permissions().mode() & 0o111 != 0 {
                                let dest = dest_bin_dir.join(entry.file_name());
                                fs_err::tokio::copy(&path, &dest).await?;
                            }
                        }
                        #[cfg(windows)]
                        {
                            if path.extension().is_some_and(|e| e == "exe") {
                                let dest = dest_bin_dir.join(entry.file_name());
                                fs_err::tokio::copy(&path, &dest).await?;
                            }
                        }
                    }
                }
            }
        }

        // Install CLI dependencies
        for cli_dep in cli_deps {
            let (package, version) = cli_dep.split_once(':').unwrap_or((cli_dep, ""));
            let mut cmd = Cmd::new("cargo", "install cli dep");
            cmd.args(["install", "--bins", "--root"])
                .arg(cargo_home)
                .arg(package);
            if !version.is_empty() {
                cmd.args(["--version", version]);
            }
            cmd.env(EnvVars::CARGO_HOME, cargo_home)
                .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                .remove_git_env()
                .check(true)
                .output()
                .await?;
        }

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, _info: &InstallInfo) -> anyhow::Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
    ) -> anyhow::Result<(i32, Vec<u8>)> {
        let env_dir = hook.env_path().expect("Rust hook must have env path");
        let info = hook.install_info().expect("Rust hook must be installed");

        let rust_bin = bin_dir(env_dir);
        let rust_tools = store.tools_path(ToolBucket::Rust);
        let rustc_bin = info.toolchain.parent().expect("Rust bin should exist");

        // Determine if this is a managed (non-system) Rust installation
        let rust_envs = if rustc_bin.starts_with(&rust_tools) {
            let toolchain = info.language_version.to_string();
            // Get the toolchain directory (parent of bin/)
            let toolchain_dir = rustc_bin.parent().expect("Toolchain dir should exist");
            let rustup_home = rustup_home_dir(toolchain_dir);
            vec![
                (EnvVars::RUSTUP_TOOLCHAIN, toolchain),
                (
                    EnvVars::RUSTUP_HOME,
                    rustup_home.to_string_lossy().to_string(),
                ),
            ]
        } else {
            vec![]
        };

        let new_path = prepend_paths(&[&rust_bin, rustc_bin]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;
        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "rust hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::CARGO_HOME, env_dir)
                .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                .envs(rust_envs.iter().map(|(k, v)| (k, v.as_str())))
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}

pub(crate) fn bin_dir(env_path: &Path) -> PathBuf {
    env_path.join("bin")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    async fn write_file(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs_err::tokio::create_dir_all(parent).await.unwrap();
        }
        fs_err::tokio::write(path, content).await.unwrap();
    }

    #[tokio::test]
    async fn test_find_package_dir_single_package() {
        let temp = TempDir::new().unwrap();
        let cargo_toml = r#"
[package]
name = "my-tool"
version = "0.1.0"
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        let (path, is_workspace) = find_package_dir(temp.path(), "my-tool").await.unwrap();
        assert_eq!(path, temp.path());
        assert!(!is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_single_package_underscore_normalization() {
        let temp = TempDir::new().unwrap();
        let cargo_toml = r#"
[package]
name = "my-tool"
version = "0.1.0"
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        // Should match with underscores instead of hyphens
        let (path, is_workspace) = find_package_dir(temp.path(), "my_tool").await.unwrap();
        assert_eq!(path, temp.path());
        assert!(!is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_workspace_with_root_package() {
        // This is the cargo-deny case: workspace where root is also a package
        let temp = TempDir::new().unwrap();
        let cargo_toml = r#"
[package]
name = "cargo-deny"
version = "0.18.5"

[workspace]
members = ["subcrate"]
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        // Create a subcrate too
        let subcrate_toml = r#"
[package]
name = "subcrate"
version = "0.1.0"
"#;
        write_file(&temp.path().join("subcrate/Cargo.toml"), subcrate_toml).await;

        let (path, is_workspace) = find_package_dir(temp.path(), "cargo-deny").await.unwrap();
        assert_eq!(path, temp.path());
        assert!(is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_workspace_member() {
        let temp = TempDir::new().unwrap();
        let cargo_toml = r#"
[workspace]
members = ["cli", "lib"]
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        let cli_toml = r#"
[package]
name = "my-cli"
version = "0.1.0"
"#;
        write_file(&temp.path().join("cli/Cargo.toml"), cli_toml).await;

        let lib_toml = r#"
[package]
name = "my-lib"
version = "0.1.0"
"#;
        write_file(&temp.path().join("lib/Cargo.toml"), lib_toml).await;

        let (path, is_workspace) = find_package_dir(temp.path(), "my-cli").await.unwrap();
        assert_eq!(path, temp.path().join("cli"));
        assert!(is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_no_cargo_toml() {
        let temp = TempDir::new().unwrap();

        let result = find_package_dir(temp.path(), "anything").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No Cargo.toml"));
    }

    #[tokio::test]
    async fn test_find_package_dir_workspace_binary_not_found() {
        let temp = TempDir::new().unwrap();
        let cargo_toml = r#"
[workspace]
members = ["cli"]
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        let cli_toml = r#"
[package]
name = "some-other-tool"
version = "0.1.0"
"#;
        write_file(&temp.path().join("cli/Cargo.toml"), cli_toml).await;

        let result = find_package_dir(temp.path(), "nonexistent-binary").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No package found"));
    }

    #[test]
    fn test_extract_package_name() {
        let content = r#"
[package]
name = "my-tool"
version = "0.1.0"
"#;
        assert_eq!(extract_package_name(content), Some("my-tool".to_string()));
    }

    #[test]
    fn test_extract_package_name_with_single_quotes() {
        let content = r"
[package]
name = 'my-tool'
version = '0.1.0'
";
        assert_eq!(extract_package_name(content), Some("my-tool".to_string()));
    }

    #[test]
    fn test_extract_package_name_no_package() {
        let content = r#"
[workspace]
members = ["cli"]
"#;
        assert_eq!(extract_package_name(content), None);
    }

    #[test]
    fn test_format_cargo_dependency() {
        assert_eq!(format_cargo_dependency("serde"), "serde@*");
        assert_eq!(format_cargo_dependency("serde:1.0"), "serde@1.0");
        assert_eq!(format_cargo_dependency("tokio:1.0.0"), "tokio@1.0.0");
    }
}
