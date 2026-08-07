use std::ffi::OsString;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageBackend;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{CacheBucket, Store};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Julia;

fn depot_path(store: &Store) -> Result<OsString> {
    let depot = store.cache_path(CacheBucket::Julia);
    std::env::join_paths([depot.as_path(), Path::new("")])
        .context("Failed to join Julia depot path")
}

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Julia {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing Julia environment");

        fs_err::tokio::create_dir_all(&info.env_path).await?;
        let depot = store.cache_path(CacheBucket::Julia);
        fs_err::tokio::create_dir_all(&depot).await?;
        let depot_path = depot_path(store)?;
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

        Cmd::new("julia")
            .current_dir(search_path)
            .arg("--startup-file=no")
            .arg(format!("--project={}", info.env_path.display()))
            .arg("-e")
            .arg(julia_code)
            .arg("--")
            .args(&hook.additional_dependencies)
            .env(EnvVars::JULIA_DEPOT_PATH, &depot_path)
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
        Cmd::new("julia")
            .arg("--version")
            .check(true)
            .output()
            .await
            .context("Julia is not available")?;
        Ok(())
    }

    async fn run(
        &self,
        store: &Store,
        hook: &InstalledHook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Julia must have env path");
        let depot_path = depot_path(store)?;

        let mut entry = hook.entry.expect_direct().split()?;
        if let Some(repo_path) = hook.repo_path() {
            let jl_path = repo_path.join(&entry[0]);
            if jl_path.exists() {
                entry[0] = jl_path.into_os_string();
            }
        }

        let run = async |batch: &[&Path]| {
            let output = Cmd::new("julia")
                .current_dir(hook.work_dir())
                .arg("--startup-file=no")
                .arg(format!("--project={}", env_dir.display()))
                .args(&entry)
                .envs(&hook.env)
                .env(EnvVars::JULIA_DEPOT_PATH, &depot_path)
                .args(&hook.args)
                .file_args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output_with_sink(reporter.output_sink(progress))
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            anyhow::Ok(output)
        };

        let output = run_by_batch(hook, filenames, &entry, run).await?;

        reporter.on_run_complete(progress);

        Ok(output)
    }
}
