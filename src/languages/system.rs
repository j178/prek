use std::collections::HashMap;

use anyhow::Result;

use crate::hook::Hook;
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;

#[derive(Debug, Copy, Clone)]
pub struct System;

impl LanguageImpl for System {
    fn supports_dependency(&self) -> bool {
        false
    }

    async fn install(&self, _hook: &Hook) -> anyhow::Result<()> {
        Ok(())
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
    ) -> Result<(i32, Vec<u8>)> {
        let cmds = shlex::split(&hook.entry).ok_or(anyhow::anyhow!("Failed to parse entry"))?;

        let run = async move |batch: Vec<String>| {
            let mut output = Cmd::new(&cmds[0], "run system command")
                .args(&cmds[1..])
                .args(&hook.args)
                .args(batch)
                .envs(env_vars)
                .check(false)
                .output()
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
