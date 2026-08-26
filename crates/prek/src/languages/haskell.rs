use std::ffi::OsString;
use std::path::Path;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use asyncband::once::OnceCell;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::git::GitCommandExt;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process::Cmd;
use crate::store::Store;

static CABAL_UPDATE_ONCE: OnceCell<()> = OnceCell::new();
static SKIP_CABAL_UPDATE: LazyLock<bool> = LazyLock::new(|| {
    EnvVars
        .var(EnvVars::PREK_INTERNAL__SKIP_CABAL_UPDATE)
        .is_ok()
});

#[derive(Debug, Copy, Clone)]
pub(crate) struct Haskell;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Haskell {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        install_cwd: &Path,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing Haskell environment");

        let bin_dir = info.env_path.join("bin");
        fs_err::tokio::create_dir_all(&bin_dir).await?;

        // Identify packages: *.cabal files in repo + additional_dependencies
        let search_path = hook.repo_path().unwrap_or_else(|| hook.project().path());
        let project_packages = fs_err::read_dir(search_path)?
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("cabal"))
                {
                    if hook.repo_path().is_some() {
                        path.file_name().map(OsString::from)
                    } else {
                        path.file_stem().map(OsString::from)
                    }
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        let additional_dependencies = hook
            .additional_dependencies
            .iter()
            .map(OsString::from)
            .collect::<Vec<_>>();

        if project_packages.is_empty() && additional_dependencies.is_empty() {
            anyhow::bail!("Expected .cabal files or additional_dependencies");
        }

        // Run `cabal update` unless explicitly skipped via PREK_INTERNAL__SKIP_CABAL_UPDATE (e.g., in CI)
        if !*SKIP_CABAL_UPDATE {
            // `cabal update` is slow, so only run it once per process.
            CABAL_UPDATE_ONCE
                .get_or_try_init(async || {
                    Cmd::new("cabal")
                        .current_dir(install_cwd)
                        .arg("update")
                        .check(true)
                        .output()
                        .await
                        .context("Failed to run `cabal update`")
                        .map(|_| ())
                })
                .await?;
        }

        if !project_packages.is_empty() {
            // Cabal does not accept an absolute `.cabal` file as an install target. Point local
            // hooks at their project explicitly while keeping the process cwd isolated.
            let project_dir = if hook.repo_path().is_some() {
                None
            } else {
                Some(search_path)
            };
            install_packages(install_cwd, &bin_dir, &project_packages, project_dir)
                .await
                .context("Failed to install Haskell hook project")?;
        }

        if !additional_dependencies.is_empty() {
            // User-supplied targets must continue to resolve from the isolated install cwd.
            install_packages(install_cwd, &bin_dir, &additional_dependencies, None)
                .await
                .context("Failed to install Haskell additional dependencies")?;
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
        let env_dir = hook.env_path().expect("Haskell must have env path");
        let new_path = prepend_paths(&[&env_dir.join("bin")]).context("Failed to join PATH")?;

        let mut environment = ExecutionEnvironment::new();
        environment.set_path(&new_path);
        Ok(environment)
    }
}

async fn install_packages(
    install_cwd: &Path,
    bin_dir: &Path,
    packages: &[OsString],
    project_dir: Option<&Path>,
) -> Result<()> {
    let mut command = Cmd::new("cabal");
    command.current_dir(install_cwd).arg("v2-install");
    if let Some(project_dir) = project_dir {
        command.arg("--project-dir").arg(project_dir);
    }
    command
        .arg("--installdir")
        .arg(bin_dir)
        .args(packages)
        .sanitize_git_repo_env()
        .check(true)
        .output()
        .await?;
    Ok(())
}
