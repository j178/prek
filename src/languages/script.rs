use std::collections::HashMap;

use anyhow::Result;

use crate::hook::Hook;
use crate::hook::ResolvedHook;
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub struct Script;

impl LanguageImpl for Script {
    fn supports_dependency(&self) -> bool {
        // As of now: "if it needs additional dependencies the consumer must install them manually."
        false
    }

    async fn resolve(&self, hook: &Hook, _store: &Store) -> Result<ResolvedHook> {
        Ok(ResolvedHook::NoNeedInstall(hook.clone()))
    }

    async fn install(&self, _hook: &ResolvedHook, _store: &Store) -> Result<()> {
        // No installation step needed for Bash scripts
        Ok(())
    }

    async fn check_health(&self) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &ResolvedHook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let cmds = shlex::split(&hook.entry).ok_or(anyhow::anyhow!("Failed to parse entry"))?;

        let run = async move |batch: Vec<String>| {
            let mut command = Cmd::new(&cmds[0], "run script command")
                .args(&cmds[1..])
                .args(&hook.args)
                .args(batch)
                .envs(env_vars)
                .output()
                .await?;

            command.stdout.extend(command.stderr);
            let code = command.status.code().unwrap_or(1);
            anyhow::Ok((code, command.stdout))
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
