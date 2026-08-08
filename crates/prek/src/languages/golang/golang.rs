use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;

use crate::cli::reporter::HookInstallReporter;
use crate::git::GitCommandExt;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::golang::GoRequest;
use crate::languages::golang::installer::GoInstaller;
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::store::{CacheBucket, Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Golang;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Golang {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        reporter: &HookInstallReporter,
    ) -> anyhow::Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        // 1. Install Go
        let go_dir = store.tools_path(ToolBucket::Go);
        let installer = GoInstaller::new(go_dir);

        let version: &GoRequest = hook.language_request.version();
        let go = installer
            .install(store, version, hook.language_request.allows_download())
            .await
            .context("Failed to install go")?;

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;
        info.with_toolchain(go.bin().to_path_buf())
            .with_language_version(go.version().deref().clone());

        // 2. Create environment
        fs_err::tokio::create_dir_all(bin_dir(&info.env_path)).await?;

        // 3. Install dependencies
        // go: ~/.cache/prek/tools/go/1.24.0/bin/go
        // go_root: ~/.cache/prek/tools/go/1.24.0
        // go_cache: ~/.cache/prek/cache/go
        // go_bin: ~/.cache/prek/hooks/envs/<hook_id>/bin
        let go_root = go
            .bin()
            .parent()
            .and_then(|p| p.parent())
            .expect("Go root should exist");
        let go_cache = store.cache_path(CacheBucket::Go);

        let go_install_cmd = || {
            if go.is_from_system() {
                let mut cmd = go.cmd();
                cmd.arg("install")
                    .env(EnvVars::GOTOOLCHAIN, "local")
                    .env(EnvVars::GOBIN, bin_dir(&info.env_path));
                cmd
            } else {
                let mut cmd = go.cmd();
                cmd.arg("install")
                    .env(EnvVars::GOTOOLCHAIN, "local")
                    .env(EnvVars::GOROOT, go_root)
                    .env(EnvVars::GOBIN, bin_dir(&info.env_path))
                    .env(EnvVars::GOFLAGS, "-modcacherw")
                    .env(EnvVars::GOPATH, &go_cache);
                cmd
            }
        };

        // GOPATH used to store downloaded source code (in $GOPATH/pkg/mod)
        if let Some(repo) = hook.repo_path() {
            go_install_cmd()
                .arg("./...")
                .current_dir(repo)
                .sanitize_git_repo_env()
                .check(true)
                .output()
                .await?;
        }
        for dep in &hook.additional_dependencies {
            let mut cmd = go_install_cmd();
            if let Some(repo) = hook.repo_path() {
                cmd.current_dir(repo);
            }
            cmd.arg(dep)
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

    async fn check_health(&self, _info: &InstallInfo) -> anyhow::Result<()> {
        Ok(())
    }

    fn execution_environment(
        &self,
        store: &Store,
        hook: &InstalledHook,
    ) -> anyhow::Result<ExecutionEnvironment> {
        let env_dir = hook.env_path().expect("Go hook must have env path");
        let go_bin = bin_dir(env_dir);
        let go_tools = store.tools_path(ToolBucket::Go);
        let go_root_bin = hook.toolchain_dir().expect("Go root should exist");
        let go_root = go_root_bin.parent().expect("Go root should exist");
        let go_cache = store.cache_path(CacheBucket::Go);
        let new_path = prepend_paths(&[&go_bin, go_root_bin]).context("Failed to join PATH")?;

        let mut environment = ExecutionEnvironment::new();
        environment
            .set_path(&new_path)
            .env(EnvVars::GOTOOLCHAIN, "local")
            .env(EnvVars::GOBIN, &go_bin)
            .env(EnvVars::GOFLAGS, "-modcacherw");
        if go_root_bin.starts_with(go_tools) {
            environment
                .env(EnvVars::GOROOT, go_root)
                .env(EnvVars::GOPATH, &go_cache);
        }
        Ok(environment)
    }
}

pub(crate) fn bin_dir(env_path: &Path) -> PathBuf {
    env_path.join("bin")
}
