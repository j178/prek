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

/// Check if a package produces a binary with the given name.
/// This checks:
/// 1. `[[bin]]` entries with explicit `name`
/// 2. Files in `src/bin/*.rs`
/// 3. Package name (default binary name, only if src/main.rs exists)
fn package_produces_binary(content: &str, package_dir: &Path, binary_name: &str) -> bool {
    // Check [[bin]] entries first - these are explicit binary definitions
    for bin_name in extract_bin_names(content) {
        if names_match(&bin_name, binary_name) {
            return true;
        }
    }

    // Check src/bin/*.rs files (each produces a binary named after the file)
    let bin_dir = package_dir.join("src/bin");
    if bin_dir.exists() {
        if let Ok(entries) = std::fs::read_dir(&bin_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "rs") {
                    if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                        if names_match(stem, binary_name) {
                            return true;
                        }
                    }
                }
            }
        }
    }

    // Check package name ONLY if src/main.rs exists (default binary)
    // This must come last to avoid matching library packages
    let main_rs = package_dir.join("src/main.rs");
    if main_rs.exists() {
        if let Some(pkg_name) = extract_package_name(content) {
            if names_match(&pkg_name, binary_name) {
                return true;
            }
        }
    }

    false
}

/// Check if two names match, accounting for hyphen/underscore normalization.
fn names_match(a: &str, b: &str) -> bool {
    a == b || a.replace('-', "_") == b.replace('-', "_")
}

/// Extract binary names from `[[bin]]` sections in Cargo.toml.
fn extract_bin_names(content: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_bin_section = false;

    for line in content.lines() {
        let trimmed = line.trim();

        if trimmed == "[[bin]]" {
            in_bin_section = true;
            continue;
        }

        // Exit bin section if we hit another section
        if in_bin_section && trimmed.starts_with('[') {
            in_bin_section = false;
            continue;
        }

        if in_bin_section && trimmed.starts_with("name") {
            if let Some((_key, value)) = trimmed.split_once('=') {
                let name = value.trim().trim_matches('"').trim_matches('\'');
                names.push(name.to_string());
            }
        }
    }

    names
}

/// Find the package directory that produces the given binary.
/// Returns (`package_dir`, `package_name`, `is_workspace`).
async fn find_package_dir(
    repo: &Path,
    binary_name: &str,
) -> anyhow::Result<(PathBuf, String, bool)> {
    let root_cargo = repo.join("Cargo.toml");
    if !root_cargo.exists() {
        anyhow::bail!("No Cargo.toml found in {}", repo.display());
    }

    let content = fs_err::tokio::read_to_string(&root_cargo).await?;

    // If it's a workspace, search workspace members
    if content.contains("[workspace]") {
        // First, check if the root itself is also a package
        if content.contains("[package]") {
            if let Some(pkg_name) = extract_package_name(&content) {
                if package_produces_binary(&content, repo, binary_name) {
                    return Ok((repo.to_path_buf(), pkg_name, true));
                }
            }
        }

        // Parse workspace members and search them
        let members = parse_workspace_members(&content);
        for member_pattern in members {
            let member_paths = resolve_workspace_member(repo, &member_pattern)?;

            for member_path in member_paths {
                let member_cargo = member_path.join("Cargo.toml");
                if member_cargo.exists() {
                    let member_content = fs_err::tokio::read_to_string(&member_cargo).await?;
                    if let Some(pkg_name) = extract_package_name(&member_content) {
                        if package_produces_binary(&member_content, &member_path, binary_name) {
                            return Ok((member_path, pkg_name, true));
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
        let pkg_name = extract_package_name(&content)
            .ok_or_else(|| anyhow::anyhow!("No package name found in {}", root_cargo.display()))?;
        return Ok((repo.to_path_buf(), pkg_name, false));
    }

    anyhow::bail!("Invalid Cargo.toml in {}", repo.display());
}

/// Parse the `members` array from a workspace Cargo.toml.
/// This is a simple parser that handles the common cases.
fn parse_workspace_members(content: &str) -> Vec<String> {
    let mut members = Vec::new();
    let mut in_workspace = false;
    let mut in_members = false;
    let mut bracket_depth = 0;

    for line in content.lines() {
        let trimmed = line.trim();

        // Track when we enter [workspace] section
        if trimmed == "[workspace]" {
            in_workspace = true;
            continue;
        }

        // Exit workspace section if we hit another top-level section
        if in_workspace && trimmed.starts_with('[') && !trimmed.starts_with("[[") {
            in_workspace = false;
            in_members = false;
            continue;
        }

        if !in_workspace {
            continue;
        }

        // Look for members = [...]
        if trimmed.starts_with("members") {
            if let Some(rest) = trimmed.strip_prefix("members").map(str::trim) {
                if let Some(rest) = rest.strip_prefix('=').map(str::trim) {
                    // Check if it's a single-line array
                    if let Some(rest_after_bracket) = rest.strip_prefix('[') {
                        if rest.ends_with(']') {
                            // Single line: members = ["a", "b"]
                            let inner = rest_after_bracket;
                            members.extend(parse_string_array(inner));
                        } else {
                            // Multi-line array starts here
                            in_members = true;
                            bracket_depth = 1;
                            let inner = &rest[1..];
                            members.extend(parse_string_array(inner));
                        }
                    }
                }
            }
            continue;
        }

        // Continue parsing multi-line members array
        if in_members {
            if trimmed.contains(']') {
                bracket_depth -= 1;
                if bracket_depth == 0 {
                    // Parse content before the closing bracket
                    if let Some(idx) = trimmed.find(']') {
                        members.extend(parse_string_array(&trimmed[..idx]));
                    }
                    in_members = false;
                }
            } else {
                members.extend(parse_string_array(trimmed));
            }
        }
    }

    members
}

/// Parse comma-separated quoted strings from a line.
fn parse_string_array(line: &str) -> Vec<String> {
    let mut results = Vec::new();
    let mut in_string = false;
    let mut quote_char = '"';
    let mut current = String::new();

    for ch in line.chars() {
        if !in_string {
            if ch == '"' || ch == '\'' {
                in_string = true;
                quote_char = ch;
                current.clear();
            }
        } else if ch == quote_char {
            in_string = false;
            if !current.is_empty() {
                results.push(current.clone());
            }
        } else {
            current.push(ch);
        }
    }

    results
}

/// Resolve a workspace member pattern to actual paths.
/// Handles both direct paths (e.g., "crates/cli") and globs (e.g., "crates/*").
fn resolve_workspace_member(repo: &Path, pattern: &str) -> anyhow::Result<Vec<PathBuf>> {
    let full_pattern = repo.join(pattern);

    // Check if it's a glob pattern
    if pattern.contains('*') || pattern.contains('?') || pattern.contains('[') {
        let pattern_str = full_pattern.to_string_lossy();
        let paths: Vec<PathBuf> = glob::glob(&pattern_str)
            .map_err(|e| anyhow::anyhow!("Invalid glob pattern '{pattern}': {e}"))?
            .filter_map(std::result::Result::ok)
            .filter(|p| p.is_dir())
            .collect();
        Ok(paths)
    } else {
        // Direct path
        let path = repo.join(pattern);
        if path.exists() {
            Ok(vec![path])
        } else {
            Ok(vec![])
        }
    }
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
            let (package_dir, package_name, is_workspace) =
                find_package_dir(repo, binary_name).await?;

            if lib_deps.is_empty() && !is_workspace {
                // For single packages without lib deps, use cargo install directly
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
            } else if lib_deps.is_empty() {
                // For workspace members without lib deps, use cargo build + copy
                // (cargo install doesn't work well with virtual workspaces)
                let target_dir = info.env_path.join("target");
                Cmd::new("cargo", "build local")
                    .args(["build", "--bins", "--release"])
                    .arg("--manifest-path")
                    .arg(package_dir.join("Cargo.toml"))
                    .arg("--target-dir")
                    .arg(&target_dir)
                    .current_dir(repo)
                    .env(EnvVars::CARGO_HOME, cargo_home)
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
                let mut cmd = Cmd::new("cargo", "build local with deps");
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
                    .env(EnvVars::CARGO_HOME, cargo_home)
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
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        // Should match with underscores instead of hyphens
        let (path, _pkg, is_workspace) = find_package_dir(temp.path(), "my_tool").await.unwrap();
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
        // Create src/main.rs so it's detected as a binary package
        write_file(&temp.path().join("src/main.rs"), "fn main() {}").await;

        // Create a subcrate too
        let subcrate_toml = r#"
[package]
name = "subcrate"
version = "0.1.0"
"#;
        write_file(&temp.path().join("subcrate/Cargo.toml"), subcrate_toml).await;

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
"#;
        write_file(&temp.path().join("cli/Cargo.toml"), cli_toml).await;
        // Create src/main.rs so it's detected as a binary package
        write_file(&temp.path().join("cli/src/main.rs"), "fn main() {}").await;

        let lib_toml = r#"
[package]
name = "my-lib"
version = "0.1.0"
"#;
        write_file(&temp.path().join("lib/Cargo.toml"), lib_toml).await;

        let (path, pkg_name, is_workspace) = find_package_dir(temp.path(), "my-cli").await.unwrap();
        assert_eq!(path, temp.path().join("cli"));
        assert_eq!(pkg_name, "my-cli");
        assert!(is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_by_bin_name() {
        // Package name differs from binary name
        let temp = TempDir::new().unwrap();

        let cargo_toml = r#"
[workspace]
members = ["crates/*"]
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;

        // Package is typos-cli but binary is typos
        let cli_toml = r#"
[package]
name = "typos-cli"
version = "0.1.0"

[[bin]]
name = "typos"
path = "src/main.rs"
"#;
        write_file(&temp.path().join("crates/typos-cli/Cargo.toml"), cli_toml).await;

        // Should find by binary name, return package name
        let (path, pkg_name, is_workspace) = find_package_dir(temp.path(), "typos").await.unwrap();
        assert_eq!(path, temp.path().join("crates/typos-cli"));
        assert_eq!(pkg_name, "typos-cli"); // Package name, not binary name
        assert!(is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_by_src_bin_file() {
        // Binary defined by src/bin/foo.rs
        let temp = TempDir::new().unwrap();

        let cargo_toml = r#"
[package]
name = "my-pkg"
version = "0.1.0"
"#;
        write_file(&temp.path().join("Cargo.toml"), cargo_toml).await;
        write_file(&temp.path().join("src/bin/my-tool.rs"), "fn main() {}").await;

        let (path, _pkg, is_workspace) = find_package_dir(temp.path(), "my-tool").await.unwrap();
        assert_eq!(path, temp.path());
        assert!(!is_workspace);
    }

    #[test]
    fn test_extract_bin_names() {
        let content = r#"
[package]
name = "typos-cli"

[[bin]]
name = "typos"
path = "src/main.rs"

[[bin]]
name = "typos-other"
path = "src/other.rs"
"#;
        let names = extract_bin_names(content);
        assert_eq!(names, vec!["typos", "typos-other"]);
    }

    #[test]
    fn test_extract_bin_names_empty() {
        let content = r#"
[package]
name = "simple"
"#;
        let names = extract_bin_names(content);
        assert!(names.is_empty());
    }

    #[tokio::test]
    async fn test_find_package_dir_virtual_workspace_nested_member() {
        // Virtual workspace: root has [workspace] only, members are nested
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
"#;
        write_file(&temp.path().join("crates/cli/Cargo.toml"), cli_toml).await;
        // Create src/main.rs so it's detected as a binary package
        write_file(&temp.path().join("crates/cli/src/main.rs"), "fn main() {}").await;

        let (path, pkg_name, is_workspace) =
            find_package_dir(temp.path(), "virtual-cli").await.unwrap();
        assert_eq!(path, temp.path().join("crates/cli"));
        assert_eq!(pkg_name, "virtual-cli");
        assert!(is_workspace);
    }

    #[tokio::test]
    async fn test_find_package_dir_virtual_workspace_glob_members() {
        // Virtual workspace with glob pattern
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
"#;
        write_file(&temp.path().join("crates/cli/Cargo.toml"), cli_toml).await;
        // Create src/main.rs so it's detected as a binary package
        write_file(&temp.path().join("crates/cli/src/main.rs"), "fn main() {}").await;

        let lib_toml = r#"
[package]
name = "my-lib"
version = "0.1.0"
"#;
        write_file(&temp.path().join("crates/lib/Cargo.toml"), lib_toml).await;

        let (path, pkg_name, is_workspace) = find_package_dir(temp.path(), "my-cli").await.unwrap();
        assert_eq!(path, temp.path().join("crates/cli"));
        assert_eq!(pkg_name, "my-cli");
        assert!(is_workspace);

        // my-lib is a library (no main.rs), so searching for it as a binary should fail
        let result = find_package_dir(temp.path(), "my-lib").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_workspace_members_single_line() {
        let content = r#"
[workspace]
members = ["crates/cli", "crates/lib"]
"#;
        let members = parse_workspace_members(content);
        assert_eq!(members, vec!["crates/cli", "crates/lib"]);
    }

    #[test]
    fn test_parse_workspace_members_multi_line() {
        let content = r#"
[workspace]
members = [
    "crates/cli",
    "crates/lib",
]
"#;
        let members = parse_workspace_members(content);
        assert_eq!(members, vec!["crates/cli", "crates/lib"]);
    }

    #[test]
    fn test_parse_workspace_members_with_glob() {
        let content = r#"
[workspace]
members = ["crates/*", "tools/build"]
"#;
        let members = parse_workspace_members(content);
        assert_eq!(members, vec!["crates/*", "tools/build"]);
    }

    #[test]
    fn test_parse_workspace_members_workspace_after_package() {
        // Some crates have [package] before [workspace]
        let content = r#"
[package]
name = "root-crate"
version = "0.1.0"

[workspace]
members = ["subcrate"]
"#;
        let members = parse_workspace_members(content);
        assert_eq!(members, vec!["subcrate"]);
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
