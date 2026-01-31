use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use mea::once::OnceCell;
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

static CABAL_UPDATE_ONCE: OnceCell<()> = OnceCell::new();
static SKIP_CABAL_UPDATE: LazyLock<bool> =
    LazyLock::new(|| EnvVars::var(EnvVars::PREK_INTERNAL__SKIP_CABAL_UPDATE).is_ok());

#[derive(Debug, Copy, Clone)]
pub(crate) struct Haskell;

impl LanguageImpl for Haskell {
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

        debug!(%hook, target = %info.env_path.display(), "Installing Haskell environment");

        let bin_dir = info.env_path.join("bin");
        fs_err::tokio::create_dir_all(&bin_dir).await?;

        // Identify packages: *.cabal files in repo + additional_dependencies
        let search_path = hook.repo_path().unwrap_or(hook.project().path());
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
                        .map(|name| name.to_string_lossy().to_string())
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
                .get_or_try_init(|| async {
                    Cmd::new("cabal", "update cabal package database")
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
        Cmd::new("cabal", "install haskell dependencies")
            .current_dir(search_path)
            .arg("v2-install")
            .arg("--installdir")
            .arg(&bin_dir)
            .args(pkgs)
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

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Haskell must have env path");
        let bin_dir = env_dir.join("bin");
        let new_path = prepend_paths(&[&bin_dir]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run haskell hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
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

        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}
