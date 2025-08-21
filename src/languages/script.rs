use std::sync::Arc;

use anyhow::Result;

use crate::cli::reporter::HookInstallReporter;
use crate::fs::CWD;
use crate::hook::InstalledHook;
use crate::hook::{Error, Hook};
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

    async fn check_health(&self) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        // For `language: script`, the entry must be resolved from the repo path, all other languages
        // are resolved from the current working directory.
        let repo_path = hook.repo_path().unwrap_or_else(|| CWD.as_path());
        let mut split = shlex::split(hook.entry.entry()).ok_or_else(|| Error::Hook {
            hook: hook.to_string(),
            error: anyhow::anyhow!(
                "Failed to parse entry `{}` as commands",
                &hook.entry.entry()
            ),
        })?;

        let cmd = repo_path.join(&split[0]);
        split[0] = cmd.to_string_lossy().to_string();
        let entry = resolve_command(split, None);

        let run = async move |batch: Vec<String>| {
            let mut output = Cmd::new(&entry[0], "run script command")
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
