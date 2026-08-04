use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Conda;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Conda {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing Conda environment");
        let conda = conda_executable();

        if let Some(repo_path) = hook.repo_path() {
            Cmd::new(conda)
                .current_dir(repo_path)
                .arg("create")
                .arg("-p")
                .arg(&info.env_path)
                .arg("--file")
                .arg("environment.yml")
                .check(true)
                .output()
                .await
                .context("Failed to create Conda environment")?;
        } else {
            Cmd::new(conda)
                .arg("create")
                .arg("-p")
                .arg(&info.env_path)
                .check(true)
                .output()
                .await
                .context("Failed to create Conda environment")?;
        }

        if !hook.additional_dependencies.is_empty() {
            let mut install_cmd = Cmd::new(conda);
            install_cmd
                .arg("install")
                .arg("-p")
                .arg(&info.env_path)
                .args(&hook.additional_dependencies);
            if let Some(repo_path) = hook.repo_path() {
                install_cmd.current_dir(repo_path);
            }
            install_cmd
                .check(true)
                .output()
                .await
                .context("Failed to install Conda dependencies")?;
        }

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    fn execution_environment(
        &self,
        _store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        let env_dir = hook.env_path().expect("Conda must have env path");
        let new_path = conda_path(env_dir).context("Failed to join PATH")?;

        let mut environment = ExecutionEnvironment::new();
        environment
            .set_path(&new_path)
            .env(EnvVars::CONDA_PREFIX, env_dir)
            .env_remove(EnvVars::PYTHONHOME)
            .env_remove(EnvVars::VIRTUAL_ENV);
        Ok(environment)
    }
}

fn conda_executable() -> &'static str {
    if EnvVars.is_set(EnvVars::PRE_COMMIT_USE_MICROMAMBA) {
        "micromamba"
    } else if EnvVars.is_set(EnvVars::PRE_COMMIT_USE_MAMBA) {
        "mamba"
    } else {
        "conda"
    }
}

fn conda_path(env_path: &Path) -> Result<std::ffi::OsString, std::env::JoinPathsError> {
    let paths = conda_path_dirs(env_path);
    let paths = paths.iter().map(PathBuf::as_path).collect::<Vec<_>>();
    prepend_paths(&paths)
}

fn conda_path_dirs(env_path: &Path) -> Vec<PathBuf> {
    if cfg!(windows) {
        vec![
            env_path.join("Library").join("bin"),
            env_path.join("Scripts"),
            env_path.to_path_buf(),
            env_path.join("bin"),
        ]
    } else {
        vec![env_path.join("bin")]
    }
}
