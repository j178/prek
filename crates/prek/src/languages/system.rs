use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Result;

use crate::cli::reporter::HookInstallReporter;
use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageBackend;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct System;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for System {
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
        store: &Store,
        hook: &InstalledHook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let entry = hook.entry.resolve(None, store)?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0])
                .current_dir(hook.work_dir())
                .envs(&hook.env)
                .args(&entry[1..])
                .args(&hook.args)
                .file_args(batch)
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

        // Collect results
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
