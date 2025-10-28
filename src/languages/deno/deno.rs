use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use constants::env_vars::EnvVars;
use semver::Version;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::{prepend_paths, run_by_batch};
use crate::store::{CacheBucket, Store};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Deno;

pub(crate) struct DenoInfo {
    pub(crate) version: Version,
    pub(crate) executable: PathBuf,
}

async fn query_deno_info() -> Result<DenoInfo> {
    let deno_executable = which::which("deno").context("Failed to find deno executable")?;

    let stdout = Cmd::new(&deno_executable, "get deno version")
        .arg("--version")
        .check(true)
        .output()
        .await?
        .stdout;
    // deno 1.34.3 (release, x86_64-unknown-linux-gnu, linux)
    let version = String::from_utf8_lossy(&stdout)
        .split_whitespace()
        .nth(1)
        .context("Failed to get Deno version")?
        .parse::<Version>()
        .context("Failed to parse Deno version")?;

    Ok(DenoInfo {
        version,
        executable: deno_executable,
    })
}

impl LanguageImpl for Deno {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // Create env for isolated dependencies
        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;

        let deno_dir = store.cache_path(CacheBucket::Deno);
        fs_err::tokio::create_dir_all(&deno_dir).await?;

        let DenoInfo {
            version: deno_version,
            executable: deno_executable,
        } = query_deno_info().await?;

        // TODO: how to install the local repo?
        // if let Some(repo_path) = hook.repo_path() {
        //     Cmd::new(&deno_executable, "deno add")
        //         .current_dir(&info.env_path)
        //         .env(EnvVars::DENO_DIR, &deno_dir)
        //         .arg("add")
        //         .arg(format!("npm:{}", crate::fs::relative_to(repo_path, &info.env_path)?.display()))
        //         .check(true)
        //         .output()
        //         .await?;
        // }

        Cmd::new(&deno_executable, "deno add")
            .current_dir(&info.env_path)
            .env(EnvVars::DENO_DIR, &deno_dir)
            .arg("add")
            .args(&hook.additional_dependencies)
            .check(true)
            .output()
            .await?;

        info.with_language_version(deno_version)
            .with_toolchain(deno_executable);

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current = query_deno_info()
            .await
            .context("Failed to query current Deno info")?;

        if current.version != info.language_version {
            anyhow::bail!(
                "Deno version mismatch: expected `{}`, found `{}`",
                info.language_version,
                current.version
            );
        }
        if current.executable != info.toolchain {
            anyhow::bail!(
                "Deno executable mismatch: expected `{}`, found `{}`",
                info.toolchain.display(),
                current.executable.display()
            );
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let env_dir = hook.env_path().expect("Deno must have env path");
        let bin_dir = env_dir.join("bin");
        let deno_dir = store.cache_path(CacheBucket::Deno);
        let new_path = prepend_paths(&[&bin_dir]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(None)?;
        let run = async move |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "deno run")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::DENO_DIR, &deno_dir)
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

        // Collect results
        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}
