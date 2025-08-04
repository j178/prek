use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::hook::{Hook, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::docker::Docker;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct DockerImage;

impl LanguageImpl for DockerImage {
    async fn install(&self, hook: Arc<Hook>, _store: &Store) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let entry = hook.entry.parsed()?;
        let run = async move |batch: Vec<String>| {
            let mut cmd = Docker::docker_run_cmd().await?;
            let cmd = cmd
                .args(&entry[..])
                .args(&hook.args)
                .args(batch)
                .check(false)
                .envs(env_vars);

            let mut output = cmd.output().await?;
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
