use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use mea::once::OnceCell;
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
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing Haskell environment");

        let bin_dir = info.env_path.join("bin");
        fs_err::tokio::create_dir_all(&bin_dir).await?;

        // Identify packages: *.cabal files in repo + additional_dependencies
        let search_path = hook.repo_path().unwrap_or_else(|| hook.project().path());
        let pkgs = fs_err::read_dir(search_path)?
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                if path.is_file()
                    && path
                        .extension()
                        .is_some_and(|ext| ext.eq_ignore_ascii_case("cabal"))
                {
                    path.file_name()
                        .map(|name| name.to_string_lossy().into_owned())
                } else {
                    None
                }
            })
            .chain(hook.additional_dependencies.iter().cloned())
            .collect::<Vec<_>>();

        if pkgs.is_empty() {
            anyhow::bail!("Expected .cabal files or additional_dependencies");
        }

        // Run `cabal update` unless explicitly skipped via PREK_INTERNAL__SKIP_CABAL_UPDATE (e.g., in CI)
        if !*SKIP_CABAL_UPDATE {
            // `cabal update` is slow, so only run it once per process.
            CABAL_UPDATE_ONCE
                .get_or_try_init(async || {
                    Cmd::new("cabal")
                        .arg("update")
                        .check(true)
                        .output()
                        .await
                        .context("Failed to run `cabal update`")
                        .map(|_| ())
                })
                .await?;
        }

        // cabal v2-install --installdir <bindir> <pkgs> (default install-method is copy)
        Cmd::new("cabal")
            .current_dir(search_path)
            .arg("v2-install")
            .arg("--installdir")
            .arg(&bin_dir)
            .args(pkgs)
            .sanitize_git_repo_env()
            .check(true)
            .output()
            .await
            .context("Failed to install haskell dependencies")?;

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
