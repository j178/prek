use std::env::consts::EXE_EXTENSION;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::LanguageImpl;
use crate::languages::bun::BunRequest;
use crate::languages::bun::installer::{BunInstaller, BunResult, bin_dir, lib_dir};
use crate::languages::version::LanguageRequest;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Bun;

impl LanguageImpl for Bun {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install bun
        //   1) Find from `$PREK_HOME/tools/bun`
        //   2) Find from system
        //   3) Download from remote
        // 2. Create env
        // 3. Install dependencies

        // 1. Install bun
        let bun_dir = store.tools_path(ToolBucket::Bun);
        let installer = BunInstaller::new(bun_dir);

        let (bun_request, allows_download) = match &hook.language_request {
            LanguageRequest::Any { system_only } => (&BunRequest::Any, !system_only),
            LanguageRequest::Bun(bun_request) => (bun_request, true),
            _ => unreachable!(),
        };
        let bun = installer
            .install(store, bun_request, allows_download)
            .await
            .context("Failed to install bun")?;

        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        info.with_toolchain(bun.bun().to_path_buf());
        // BunVersion implements Deref<Target = semver::Version>, so we clone the inner version
        info.with_language_version((**bun.version()).clone());

        // 2. Create env
        let bin_dir = bin_dir(&info.env_path);
        let lib_dir = lib_dir(&info.env_path);
        fs_err::tokio::create_dir_all(&bin_dir).await?;
        fs_err::tokio::create_dir_all(&lib_dir).await?;

        // Create symlink or copy on Windows
        crate::fs::create_symlink_or_copy(
            bun.bun(),
            &bin_dir.join("bun").with_extension(EXE_EXTENSION),
        )
        .await?;

        // 3. Install dependencies
        let deps = hook.install_dependencies();
        if deps.is_empty() {
            debug!("No dependencies to install");
        } else {
            // `bun` needs to be in PATH for shebang scripts
            let new_path = prepend_paths(&[&bin_dir]).context("Failed to join PATH")?;

            // Use BUN_INSTALL to set where global packages are installed
            // This makes `bun install -g` install to our hook environment
            Cmd::new(bun.bun(), "bun install")
                .arg("install")
                .arg("-g")
                .args(&*deps)
                .env(EnvVars::PATH, new_path)
                .env(EnvVars::BUN_INSTALL, &info.env_path)
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

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let bun = BunResult::from_executable(info.toolchain.clone())
            .fill_version()
            .await
            .context("Failed to query bun version")?;

        if **bun.version() != info.language_version {
            anyhow::bail!(
                "Bun version mismatch: expected {}, found {}",
                info.language_version,
                bun.version()
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

        let env_dir = hook.env_path().expect("Bun must have env path");
        let new_path = prepend_paths(&[&bin_dir(env_dir)]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;
        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "bun hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::BUN_INSTALL, env_dir)
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
