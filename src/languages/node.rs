use crate::config;
use crate::hook::Hook;
use crate::languages::{LanguageImpl, DEFAULT_VERSION};

#[derive(Debug, Copy, Clone)]
pub struct Node;

impl LanguageImpl for Node {
    fn name(&self) -> config::Language {
        config::Language::Node
    }

    fn default_version(&self) -> &str {
        DEFAULT_VERSION
    }

    fn environment_dir(&self) -> Option<&str> {
        Some("node_env")
    }

    async fn install(&self, hook: &Hook) -> anyhow::Result<()> {
        // TODO: install node automatically
        let env = hook.environment_dir().expect("No environment dir found");
        fs_err::create_dir_all(env)?;
        Ok(())
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        todo!()
    }

    async fn run(&self, _hook: &Hook, _filenames: &[&String]) -> anyhow::Result<(i32, Vec<u8>)> {
        Ok((0, Vec::new()))
    }
}
