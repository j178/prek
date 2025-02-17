use std::collections::HashMap;
use std::sync::Arc;

use crate::hook::Hook;
use crate::languages::LanguageImpl;

#[derive(Debug, Copy, Clone)]
pub struct Node;

impl LanguageImpl for Node {
    fn supports_dependency(&self) -> bool {
        true
    }

    async fn install(&self, hook: &Hook) -> anyhow::Result<()> {
        // TODO: install node automatically
        let Some(env) = hook.env_path() else {
            return Ok(());
        };

        fs_err::create_dir_all(env)?;
        Ok(())
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        todo!()
    }

    async fn run(
        &self,
        _hook: &Hook,
        _filenames: &[&String],
        _env_vars: Arc<HashMap<&'static str, String>>,
    ) -> anyhow::Result<(i32, Vec<u8>)> {
        Ok((0, Vec::new()))
    }
}
