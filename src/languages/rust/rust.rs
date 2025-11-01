use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use constants::env_vars::EnvVars;
use itertools::{Either, Itertools};

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::rust::RustRequest;
use crate::languages::rust::installer::RustInstaller;
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

        // Install library dependencies
        if !lib_deps.is_empty() {
            if let Some(repo) = hook.repo_path() {
                let mut cmd = Cmd::new("cargo", "add dependencies");
                cmd.arg("add");
                for dep in &lib_deps {
                    cmd.arg(format_cargo_dependency(dep.as_str()));
                }
                cmd.current_dir(repo)
                    .env(EnvVars::CARGO_HOME, cargo_home)
                    .remove_git_env()
                    .check(true)
                    .output()
                    .await?;
            }
        }

        // Install local project
        if let Some(repo) = hook.repo_path() {
            Cmd::new("cargo", "install local")
                .args(["install", "--bins", "--root"])
                .arg(cargo_home)
                .args(["--path", "."])
                .current_dir(repo)
                .env(EnvVars::CARGO_HOME, cargo_home)
                .remove_git_env()
                .check(true)
                .output()
                .await?;
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
                .remove_git_env()
                .check(true)
                .output()
                .await?;
        }

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

        // Only set RUSTUP_TOOLCHAIN if using prek-installed Rust (not system)
        let rust_envs = if rustc_bin.starts_with(rust_tools) {
            // Use the stored version as the toolchain specifier
            let toolchain = info.language_version.to_string();
            vec![(EnvVars::RUSTUP_TOOLCHAIN, toolchain)]
        } else {
            vec![]
        };

        let new_path = prepend_paths(&[&rust_bin, rustc_bin]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;
        let run = async move |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "rust hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::CARGO_HOME, env_dir)
                .envs(rust_envs.iter().map(|(k, v)| (k, v.as_str())))
                .args(&hook.args)
                .args(batch)
                .check(false)
                .pty_output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, run).await?;

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
