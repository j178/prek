use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::debug;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Julia;

impl LanguageImpl for Julia {
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

        debug!(%hook, target = %info.env_path.display(), "Installing Julia environment");

        fs_err::tokio::create_dir_all(&info.env_path).await?;
        let search_path = hook.repo_path().unwrap_or_else(|| hook.work_dir());

        let find_src = |names: &[&str]| {
            names
                .iter()
                .map(|n| search_path.join(n))
                .find(|p| p.exists())
        };

        // Copy Project.toml if exists
        let project_dest = info.env_path.join("Project.toml");
        if let Some(src) = find_src(&["JuliaProject.toml", "Project.toml"]) {
            fs_err::tokio::copy(src, project_dest).await?;
        } else {
            // Create an empty file to ensure this is a Julia project
            fs_err::tokio::File::create(project_dest).await?;
        }

        // Copy Manifest.toml (lock) if exists
        if let Some(src) = find_src(&["JuliaManifest.toml", "Manifest.toml"]) {
            fs_err::tokio::copy(src, info.env_path.join("Manifest.toml")).await?;
        }

        let julia_code = indoc::indoc! {r"
            using Pkg
            Pkg.instantiate()
            if !isempty(ARGS)
                Pkg.add(ARGS)
            end
        "};

        Cmd::new("julia", "instantiate julia environment")
            .current_dir(search_path)
            .arg("--startup-file=no")
            .arg(format!("--project={}", info.env_path.display()))
            .arg("-e")
            .arg(julia_code)
            .arg("--")
            .args(&hook.additional_dependencies)
            .check(true)
            .output()
            .await
            .context("Failed to instantiate Julia environment")?;

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Cmd::new("julia", "check julia version")
            .arg("--version")
            .check(true)
            .output()
            .await
            .context("Julia is not available")?;
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

        let env_dir = hook.env_path().expect("Julia must have env path");
        let entry = hook.entry.resolve(None)?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new("julia", "run julia hook")
                .current_dir(hook.work_dir())
                .arg("--startup-file=no")
                .arg(format!("--project={}", env_dir.display()))
                .args(&entry)
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
