use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

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

        let info = InstallInfo::new(
            hook.language,
            hook.env_key_dependencies().clone(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %info.env_path.display(), "Installing Haskell environment");

        let bindir = info.env_path.join("bin");
        std::fs::create_dir_all(&bindir).context("Failed to create bin directory")?;

        // Identify packages: *.cabal files in repo + additional_dependencies
        let mut pkgs = Vec::new();
        let search_path = hook.repo_path().unwrap_or(hook.project().path());
        if let Ok(entries) = std::fs::read_dir(search_path) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|s| s.to_str()) == Some("cabal") {
                    pkgs.push(path.display().to_string());
                }
            }
        }
        pkgs.extend(hook.additional_dependencies.clone());

        if pkgs.is_empty() {
            anyhow::bail!("Expected .cabal files or additional_dependencies");
        }

        // cabal update
        Cmd::new("cabal", "update cabal package database")
            .arg("update")
            .check(true)
            .output()
            .await
            .context("Failed to run cabal update")?;

        // cabal install --install-method copy --installdir <bindir> <pkgs>
        Cmd::new("cabal", "install haskell dependencies")
            .current_dir(search_path)
            .arg("install")
            .arg("--install-method")
            .arg("copy")
            .arg("--installdir")
            .arg(&bindir)
            .args(pkgs)
            .check(true)
            .output()
            .await
            .context("Failed to install haskell dependencies")?;

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        // Check if cabal is installed
        Cmd::new("cabal", "check cabal version")
            .arg("--version")
            .check(true)
            .output()
            .await
            .context("cabal not found or failed to run")?;
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
        let bindir = env_dir.join("bin");
        let new_path = prepend_paths(&[&bindir]).context("Failed to join PATH")?;

        let entry = hook.entry.resolve(Some(&new_path))?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run haskell hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::PATH, &new_path)
                .args(&hook.args)
                .args(batch)
                .check(false)
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
