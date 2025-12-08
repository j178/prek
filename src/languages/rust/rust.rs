use std::env::consts::EXE_EXTENSION;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Context;
use cargo_metadata::{MetadataCommand, Package};
use itertools::{Either, Itertools};
use prek_consts::env_vars::EnvVars;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::rust::RustRequest;
use crate::languages::rust::installer::RustInstaller;
use crate::languages::rust::rustup::Rustup;
use crate::languages::rust::version::EXTRA_KEY_CHANNEL;
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::{CacheBucket, Store, ToolBucket};

fn format_cargo_dependency(dep: &str) -> String {
    let (name, version) = dep.split_once(':').unwrap_or((dep, ""));
    if version.is_empty() {
        format!("{name}@*")
    } else {
        format!("{name}@{version}")
    }
}

/// Find the package directory that produces the given binary.
/// Returns (`package_dir`, `package_name`, `is_workspace`).
async fn find_package_dir(
    repo: &Path,
    binary_name: &str,
) -> anyhow::Result<(PathBuf, String, bool)> {
    let repo = repo.to_path_buf();
    let binary_name = binary_name.to_string();

    tokio::task::spawn_blocking(move || {
        let metadata = MetadataCommand::new()
            .manifest_path(repo.join("Cargo.toml"))
            .no_deps()
            .exec()
            .context("Failed to run cargo metadata")?;

        // Search all workspace packages for one that produces this binary
        for package_id in &metadata.workspace_members {
            let package = metadata
                .packages
                .iter()
                .find(|p| &p.id == package_id)
                .ok_or_else(|| anyhow::anyhow!("Package not found in metadata"))?;

            if package_produces_binary(package, &binary_name) {
                let package_dir = package
                    .manifest_path
                    .parent()
                    .expect("manifest should have parent")
                    .as_std_path()
                    .to_path_buf();

                // It's a workspace if either:
                // - there are multiple members, OR
                // - the package is not at the workspace root
                let is_workspace = metadata.workspace_members.len() > 1
                    || package_dir != metadata.workspace_root.as_std_path();

                return Ok((package_dir, package.name.to_string(), is_workspace));
            }
        }

        anyhow::bail!(
            "No package found for binary '{}' in {}",
            binary_name,
            repo.display()
        )
    })
    .await?
}

/// Check if two names match, accounting for hyphen/underscore normalization.
fn names_match(a: &str, b: &str) -> bool {
    a == b || a.replace('-', "_") == b.replace('-', "_")
}

/// Check if a package produces a binary with the given name.
fn package_produces_binary(package: &Package, binary_name: &str) -> bool {
    package
        .targets
        .iter()
        .filter(|t| t.is_bin())
        .any(|t| names_match(&t.name, binary_name))
}

/// Copy executable binaries from a release directory to a destination bin directory.
async fn copy_binaries(release_dir: &Path, dest_bin_dir: &Path) -> anyhow::Result<()> {
    let mut entries = fs_err::tokio::read_dir(release_dir).await?;
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
    Ok(())
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
        let cargo_home = store.cache_path(CacheBucket::Cargo);
        let rustup_dir = store.tools_path(ToolBucket::Rustup);
        let rustup = Rustup::install(store, &rustup_dir).await?;
        let installer = RustInstaller::new(rustup);

        let (version, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&RustRequest::Any, !system_only),
            LanguageRequest::Rust(version) => (version, true),
            _ => unreachable!(),
        };

        let rust = installer
            .install(version, allows_download)
            .await
            .context("Failed to install rust")?;
        let cargo = rust
            .toolchain()
            .join("bin")
            .join("cargo")
            .with_extension(EXE_EXTENSION);

        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;
        info.with_toolchain(rust.toolchain().to_path_buf())
            .with_language_version(rust.version().deref().clone());

        // Store the channel name for cache matching
        match version {
            RustRequest::Channel(channel) => {
                info.with_extra(EXTRA_KEY_CHANNEL, &channel.to_string());
            }
            RustRequest::Any => {
                // Any resolves to "stable" in resolve_version
                info.with_extra(EXTRA_KEY_CHANNEL, "stable");
            }
            _ => {}
        }

        // 2. Create environment
        fs_err::tokio::create_dir_all(bin_dir(&info.env_path)).await?;

        // 3. Install dependencies
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
            let (package_dir, package_name, is_workspace) =
                find_package_dir(repo, binary_name).await?;

            if lib_deps.is_empty() && !is_workspace {
                // For single packages without lib deps, use cargo install directly
                Cmd::new(&cargo, "install local")
                    .args(["install", "--bins", "--root"])
                    .arg(&info.env_path)
                    .args(["--path", "."])
                    .current_dir(&package_dir)
                    .env(EnvVars::CARGO_HOME, &cargo_home)
                    .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                    .remove_git_env()
                    .check(true)
                    .output()
                    .await?;
            } else if lib_deps.is_empty() {
                // For workspace members without lib deps, use cargo build + copy
                // (cargo install doesn't work well with virtual workspaces)
                let target_dir = info.env_path.join("target");
                Cmd::new(&cargo, "build local")
                    .args(["build", "--bins", "--release"])
                    .arg("--manifest-path")
                    .arg(package_dir.join("Cargo.toml"))
                    .arg("--target-dir")
                    .arg(&target_dir)
                    .current_dir(repo)
                    .env(EnvVars::CARGO_HOME, &cargo_home)
                    .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                    .remove_git_env()
                    .check(true)
                    .output()
                    .await?;

                // Copy compiled binaries to the bin directory
                copy_binaries(&target_dir.join("release"), &bin_dir(&info.env_path)).await?;
            } else {
                // For packages with lib deps, copy manifest, modify, build
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
                let mut cmd = Cmd::new(&cargo, "add dependencies");
                cmd.arg("add");
                for dep in &lib_deps {
                    cmd.arg(format_cargo_dependency(dep.as_str()));
                }
                cmd.current_dir(&manifest_dir)
                    .env(EnvVars::CARGO_HOME, &cargo_home)
                    .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                    .remove_git_env()
                    .check(true)
                    .output()
                    .await?;

                // Build using cargo build with --manifest-path pointing to modified manifest
                // but source files come from original package_dir
                let target_dir = info.env_path.join("target");
                let mut cmd = Cmd::new(&cargo, "build local with deps");
                cmd.args(["build", "--bins", "--release"])
                    .arg("--manifest-path")
                    .arg(&dst_manifest)
                    .arg("--target-dir")
                    .arg(&target_dir);

                // For workspace members, explicitly specify the package
                if is_workspace {
                    cmd.args(["--package", &package_name]);
                }

                cmd.current_dir(&package_dir)
                    .env(EnvVars::CARGO_HOME, &cargo_home)
                    .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
                    .remove_git_env()
                    .check(true)
                    .output()
                    .await?;

                // Copy compiled binaries to the bin directory
                copy_binaries(&target_dir.join("release"), &bin_dir(&info.env_path)).await?;
            }
        }

        // Install CLI dependencies
        for cli_dep in cli_deps {
            let (package, version) = cli_dep.split_once(':').unwrap_or((cli_dep, ""));
            let mut cmd = Cmd::new(&cargo, "install cli dep");
            cmd.args(["install", "--bins", "--root"])
                .arg(&info.env_path)
                .arg(package);
            if !version.is_empty() {
                cmd.args(["--version", version]);
            }
            cmd.env(EnvVars::CARGO_HOME, &cargo_home)
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
        let cargo_home = store.cache_path(CacheBucket::Cargo);
        let rustc_bin = bin_dir(&info.toolchain);

        let new_path = prepend_paths(&[&rust_bin, &rustc_bin]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;
        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "rust hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::CARGO_HOME, &cargo_home)
                .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
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
edition = "2021"
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;
        write_file(&temp.path().join("src/main.rs"), "fn main() {}").await;

        let (path, pkg_name, is_workspace) =
            find_package_dir(temp.path(), "my-tool").await.unwrap();
        assert_eq!(path, temp.path());
        assert_eq!(pkg_name, "my-tool");
        assert!(!is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_single_package_underscore_normalization() {
        let temp = TempDir::new().unwrap();
        let cargo_toml = r#"
[package]
name = "my-tool"
version = "0.1.0"
edition = "2021"
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;
        write_file(&temp.path().join("src/main.rs"), "fn main() {}").await;

        // Should match with underscores instead of hyphens
        let (path, _pkg, is_workspace) = find_package_dir(temp.path(), "my_tool").await.unwrap();
        assert_eq!(path, temp.path());
        assert!(!is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_workspace_with_root_package() {
        let temp = TempDir::new().unwrap();
        let cargo_toml = r#"
[package]
name = "cargo-deny"
version = "0.18.5"
edition = "2021"

[workspace]
members = ["subcrate"]
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;
        write_file(&temp.path().join("src/main.rs"), "fn main() {}").await;

        // Create subcrate with a lib.rs
        let subcrate_toml = r#"
[package]
name = "subcrate"
version = "0.1.0"
edition = "2021"
"#;
        write_file(&temp.path().join("subcrate/Cargo.toml"), subcrate_toml).await;
        write_file(&temp.path().join("subcrate/src/lib.rs"), "").await;

        let (path, pkg_name, is_workspace) =
            find_package_dir(temp.path(), "cargo-deny").await.unwrap();
        assert_eq!(path, temp.path());
        assert_eq!(pkg_name, "cargo-deny");
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
edition = "2021"
"#;
        write_file(&temp.path().join("cli/Cargo.toml"), cli_toml).await;
        write_file(&temp.path().join("cli/src/main.rs"), "fn main() {}").await;

        let lib_toml = r#"
[package]
name = "my-lib"
version = "0.1.0"
edition = "2021"
"#;
        write_file(&temp.path().join("lib/Cargo.toml"), lib_toml).await;
        write_file(&temp.path().join("lib/src/lib.rs"), "").await;

        let (path, pkg_name, is_workspace) = find_package_dir(temp.path(), "my-cli").await.unwrap();
        assert_eq!(path, temp.path().join("cli"));
        assert_eq!(pkg_name, "my-cli");
        assert!(is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_by_bin_name() {
        let temp = TempDir::new().unwrap();

        let cargo_toml = r#"
[workspace]
members = ["crates/typos-cli"]
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        // Package is typos-cli but binary is typos
        let cli_toml = r#"
[package]
name = "typos-cli"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "typos"
path = "src/main.rs"
"#;
        write_file(&temp.path().join("crates/typos-cli/Cargo.toml"), cli_toml).await;
        write_file(
            &temp.path().join("crates/typos-cli/src/main.rs"),
            "fn main() {}",
        )
        .await;

        // Should find by binary name, return package name
        let (path, pkg_name, is_workspace) = find_package_dir(temp.path(), "typos").await.unwrap();
        assert_eq!(path, temp.path().join("crates/typos-cli"));
        assert_eq!(pkg_name, "typos-cli");
        assert!(is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_by_src_bin_file() {
        let temp = TempDir::new().unwrap();

        let cargo_toml = r#"
[package]
name = "my-pkg"
version = "0.1.0"
edition = "2021"
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;
        write_file(&temp.path().join("src/bin/my-tool.rs"), "fn main() {}").await;
        // Need a lib.rs or main.rs for the package itself
        write_file(&temp.path().join("src/lib.rs"), "").await;

        let (path, _pkg, is_workspace) = find_package_dir(temp.path(), "my-tool").await.unwrap();
        assert_eq!(path, temp.path());
        assert!(!is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_virtual_workspace_nested_member() {
        let temp = TempDir::new().unwrap();

        let cargo_toml = r#"
[workspace]
members = ["crates/cli"]
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        let cli_toml = r#"
[package]
name = "virtual-cli"
version = "0.1.0"
edition = "2021"
"#;
        write_file(&temp.path().join("crates/cli/Cargo.toml"), cli_toml).await;
        write_file(&temp.path().join("crates/cli/src/main.rs"), "fn main() {}").await;

        let (path, pkg_name, is_workspace) =
            find_package_dir(temp.path(), "virtual-cli").await.unwrap();
        assert_eq!(path, temp.path().join("crates/cli"));
        assert_eq!(pkg_name, "virtual-cli");
        assert!(is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_virtual_workspace_glob_members() {
        let temp = TempDir::new().unwrap();

        let cargo_toml = r#"
[workspace]
members = ["crates/*"]
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        let cli_toml = r#"
[package]
name = "my-cli"
version = "0.1.0"
edition = "2021"
"#;
        write_file(&temp.path().join("crates/cli/Cargo.toml"), cli_toml).await;
        write_file(&temp.path().join("crates/cli/src/main.rs"), "fn main() {}").await;

        let lib_toml = r#"
[package]
name = "my-lib"
version = "0.1.0"
edition = "2021"
"#;
        write_file(&temp.path().join("crates/lib/Cargo.toml"), lib_toml).await;
        write_file(&temp.path().join("crates/lib/src/lib.rs"), "").await;

        let (path, pkg_name, is_workspace) = find_package_dir(temp.path(), "my-cli").await.unwrap();
        assert_eq!(path, temp.path().join("crates/cli"));
        assert_eq!(pkg_name, "my-cli");
        assert!(is_workspace);

        // my-lib is a library (no binary), so searching for it should fail
        let result = find_package_dir(temp.path(), "my-lib").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_find_package_dir_no_cargo_toml() {
        let temp = TempDir::new().unwrap();

        let result = find_package_dir(temp.path(), "anything").await;
        assert!(result.is_err());
        // cargo metadata gives a different error message
        assert!(result.unwrap_err().to_string().contains("cargo metadata"));
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
edition = "2021"
"#;
        write_file(&temp.path().join("cli/Cargo.toml"), cli_toml).await;
        write_file(&temp.path().join("cli/src/main.rs"), "fn main() {}").await;

        let result = find_package_dir(temp.path(), "nonexistent-binary").await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No package found"));
    }

    #[test]
    fn test_format_cargo_dependency() {
        assert_eq!(format_cargo_dependency("serde"), "serde@*");
        assert_eq!(format_cargo_dependency("serde:1.0"), "serde@1.0");
        assert_eq!(format_cargo_dependency("tokio:1.0.0"), "tokio@1.0.0");
    }
}
