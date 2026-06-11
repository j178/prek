use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Conda;

impl LanguageImpl for Conda {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %info.env_path.display(), "Installing Conda environment");
        let conda = conda_executable();

        if let Some(repo_path) = hook.repo_path() {
            Cmd::new(conda, "create conda environment")
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
            Cmd::new(conda, "create conda environment")
                .arg("create")
                .arg("-p")
                .arg(&info.env_path)
                .check(true)
                .output()
                .await
                .context("Failed to create Conda environment")?;
        }

        if !hook.additional_dependencies.is_empty() {
            let mut install_cmd = Cmd::new(conda, "install conda dependencies");
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

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Conda must have env path");
        let new_path = conda_path(env_dir).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&new_path), store)?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run conda hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .env(EnvVars::CONDA_PREFIX, env_dir)
                .env_remove(EnvVars::PYTHONHOME)
                .env_remove(EnvVars::VIRTUAL_ENV)
                .envs(&hook.env)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output_with_sink(reporter.output_sink(progress))
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        reporter.on_run_complete(progress);

        Ok((combined_status, combined_output))
    }
}

fn conda_executable() -> &'static str {
    if EnvVars::is_set(EnvVars::PRE_COMMIT_USE_MICROMAMBA) {
        "micromamba"
    } else if EnvVars::is_set(EnvVars::PRE_COMMIT_USE_MAMBA) {
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
