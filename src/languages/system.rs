use std::process::Output;
use tokio::process::Command;

use crate::config;
use crate::hook::Hook;
use crate::languages::{LanguageImpl, DEFAULT_VERSION};

#[derive(Debug, Copy, Clone)]
pub struct System;

impl LanguageImpl for System {
    fn name(&self) -> config::Language {
        config::Language::System
    }

    fn default_version(&self) -> &str {
        DEFAULT_VERSION
    }

    fn environment_dir(&self) -> Option<&str> {
        None
    }

    async fn install(&self, _hook: &Hook) -> anyhow::Result<()> {
        Ok(())
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        Ok(())
    }

    async fn run(&self, hook: &Hook, filenames: &[&String]) -> anyhow::Result<Output> {
        let cmds = shlex::split(&hook.entry).ok_or(anyhow::anyhow!("Failed to parse entry"))?;
        let output = Command::new(&cmds[0])
            .args(&cmds[1..])
            .args(&hook.args)
            .args(filenames)
            .stderr(std::process::Stdio::inherit())
            .output()
            .await?;
        Ok(output)
    }
}
