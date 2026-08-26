use std::ffi::OsStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::git::GitCommandExt;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::bun::BunRequest;
use crate::languages::bun::installer::{BunInstaller, BunResult, bin_dir, lib_dir};
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process::Cmd;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Bun;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Bun {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
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

        let bun_request: &BunRequest = hook.language_request.version();
        let bun = installer
            .install(store, bun_request, hook.language_request.toolchain_policy())
            .await
            .context("Failed to install bun")?;

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        info.with_toolchain(bun.bun().to_path_buf());
        // BunVersion implements Deref<Target = semver::Version>, so we clone the inner version
        info.with_language_version((**bun.version()).clone());

        // 2. Create env
        let bin_dir = bin_dir(&info.env_path);
        let lib_dir = lib_dir(&info.env_path);
        fs_err::tokio::create_dir_all(&bin_dir).await?;
        fs_err::tokio::create_dir_all(&lib_dir).await?;

        // 3. Install dependencies
        let mut deps: Vec<&OsStr> = Vec::with_capacity(hook.additional_dependencies.len() + 1);
        if let Some(repo_path) = hook.repo_path() {
            deps.push(repo_path.as_os_str());
        }
        deps.extend(
            hook.additional_dependencies
                .iter()
                .map(|dependency| OsStr::new(dependency.as_str())),
        );

        if deps.is_empty() {
            debug!("No dependencies to install");
        } else {
            // `bun` needs to be in PATH for shebang scripts that use `/usr/bin/env bun`
            let bun_bin = bun.bun().parent().expect("Bun binary must have parent");
            let new_path = prepend_paths(&[&bin_dir, bun_bin]).context("Failed to join PATH")?;

            // Use BUN_INSTALL to set where global packages are installed
            // This makes `bun install -g` install to our hook environment
            Cmd::new(bun.bun())
                .arg("install")
                .arg("-g")
                .args(deps)
                .env(EnvVars::PATH, new_path)
                .env(EnvVars::BUN_INSTALL, &info.env_path)
                .sanitize_git_repo_env()
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

    fn execution_environment(
        &self,
        _store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        let env_dir = hook.env_path().expect("Bun must have env path");
        let bun_bin = hook.toolchain_dir().expect("Bun binary must have parent");
        let new_path =
            prepend_paths(&[&bin_dir(env_dir), bun_bin]).context("Failed to join PATH")?;

        let mut environment = ExecutionEnvironment::new();
        environment
            .set_path(&new_path)
            .env(EnvVars::BUN_INSTALL, env_dir);
        Ok(environment)
    }
}
