use std::sync::Arc;

use anyhow::Result;

use crate::fs::CWD;
use crate::hook::Hook;
use crate::hook::InstalledHook;
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Script;

impl LanguageImpl for Script {
    async fn install(&self, hook: Arc<Hook>, _store: &Store) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let entry = hook.entry.parsed()?;
        let repo_path = hook.repo_path().unwrap_or_else(|| CWD.as_path());
        let cmd = repo_path.join(&entry[0]);

        let run = async move |batch: Vec<String>| {
            let mut output = Cmd::new(&cmd, "run script command")
                .args(&entry[1..])
                .args(&hook.args)
                .args(batch)
                .pty_output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, run).await?;

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
