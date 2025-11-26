use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::Result;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::{LanguageImpl, resolve_command};
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Script;

impl LanguageImpl for Script {
    async fn install(
        &self,
        hook: Arc<Hook>,
        _store: &Store,
        _reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        // For `language: script`, the `entry[0]` is a script path.
        // For remote hooks, the path is relative to the repo root.
        // For local hooks, the path is relative to the current working directory.

        let repo_path = hook.repo_path().unwrap_or(hook.work_dir());
        let mut split = hook.entry.split()?;

        let cmd = repo_path.join(&split[0]);
        split[0] = cmd.to_string_lossy().to_string();
        let entry = resolve_command(split, None);

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run script command")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;

        // Collect results
        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}
