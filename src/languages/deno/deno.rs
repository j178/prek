use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use constants::env_vars::EnvVars;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::LanguageImpl;
use crate::languages::deno::DenoRequest;
use crate::languages::deno::installer::{DenoInstaller, DenoResult};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{CacheBucket, Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Deno;

impl LanguageImpl for Deno {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install deno
        //   1) Find from `$PREK_HOME/tools/deno`
        //   2) Find from system
        //   3) Download from GitHub releases
        // 2. Create env
        // 3. Install dependencies

        // 1. Install deno
        let deno_dir = store.tools_path(ToolBucket::Deno);
        let installer = DenoInstaller::new(deno_dir);

        let (deno_request, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&DenoRequest::Any, !system_only),
            LanguageRequest::Deno(deno_request) => (deno_request, true),
            _ => unreachable!(),
        };

        let deno = installer
            .install(store, deno_request, allows_download)
            .await
            .context("Failed to install deno")?;

        // Create env for isolated dependencies
        let mut info = InstallInfo::new(
            hook.language,
            hook.dependencies().clone(),
            &store.hooks_dir(),
        )?;

        info.with_toolchain(deno.deno().to_path_buf());
        info.with_language_version((**deno.version()).clone());

        let deno_cache_dir = store.cache_path(CacheBucket::Deno);
        fs_err::tokio::create_dir_all(&deno_cache_dir).await?;

        // Initialize deno.json if we have dependencies to install
        if hook.repo_path().is_some() || !hook.additional_dependencies.is_empty() {
            let deno_json = info.env_path.join("deno.json");

            // Check if repo has deno.json to copy, otherwise create minimal one
            let mut needs_deno_json = true;
            if let Some(repo_path) = hook.repo_path() {
                let repo_deno_json = repo_path.join("deno.json");
                if repo_deno_json.exists() {
                    // Copy the deno.json from the repo
                    fs_err::tokio::copy(repo_deno_json, &deno_json).await?;
                    needs_deno_json = false;
                }
                // Deno can run scripts directly from the repo without installation
            }

            if needs_deno_json {
                // Create a minimal deno.json for dependency management
                fs_err::tokio::write(&deno_json, "{}").await?;
            }

            // Install additional dependencies
            if !hook.additional_dependencies.is_empty() {
                Cmd::new(deno.deno(), "deno add")
                    .current_dir(&info.env_path)
                    .env(EnvVars::DENO_DIR, &deno_cache_dir)
                    .arg("add")
                    .args(&hook.additional_dependencies)
                    .check(true)
                    .output()
                    .await?;
            }
        }

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let current = DenoResult::from_executable(info.toolchain.clone())
            .fill_version()
            .await
            .context("Failed to query current Deno info")?;

        if **current.version() != info.language_version {
            anyhow::bail!(
                "Deno version mismatch: expected `{}`, found `{}`",
                info.language_version,
                current.version()
            );
        }
        if current.deno() != info.toolchain {
            anyhow::bail!(
                "Deno executable mismatch: expected `{}`, found `{}`",
                info.toolchain.display(),
                current.deno().display()
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
        let deno_dir = store.cache_path(CacheBucket::Deno);
        let info = hook.install_info().expect("Deno hook must be installed");

        // Use the toolchain path directly as the deno executable
        let deno_bin = &info.toolchain;

        let entry = hook.entry.split()?;
        let run = async move |batch: &[&Path]| {
            // Replace "deno" with the actual path to the installed deno binary
            let command = if entry[0] == "deno" {
                deno_bin.as_path()
            } else {
                Path::new(&entry[0])
            };

            let mut output = Cmd::new(command, "deno run")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
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
