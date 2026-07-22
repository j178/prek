use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Result;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageBackend;
use crate::languages::docker::Docker;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct DockerImage;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for DockerImage {
    async fn install(
        &self,
        _store: &Store,
        hook: Arc<Hook>,
        _reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        _store: &Store,
        hook: &InstalledHook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        // Pass environment variables on the command line (they will appear in ps output).
        let env_args: Vec<String> = hook
            .env
            .iter()
            .flat_map(|(key, value)| ["-e".to_owned(), format!("{key}={value}")])
            .collect();

        let entry = hook.entry.expect_direct().split()?;
        let run = async |batch: &[&Path]| {
            let mut cmd = Docker::docker_run_cmd(hook.work_dir());
            let output = cmd
                .current_dir(hook.work_dir())
                .args(&env_args)
                .args(&entry[..])
                .args(&hook.args)
                .file_args(batch)
                .check(false)
                .stdin(Stdio::null())
                .output_with_sink(reporter.output_sink(progress))
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            anyhow::Ok(output)
        };

        let output = run_by_batch(hook, filenames, &entry, run).await?;

        reporter.on_run_complete(progress);

        Ok(output)
    }
}
