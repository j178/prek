use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::hook::Hook;
use crate::languages::node::installer::NodeInstaller;
use crate::languages::LanguageImpl;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub struct Node;

impl LanguageImpl for Node {
    fn supports_dependency(&self) -> bool {
        true
    }

    async fn install(&self, hook: &Hook) -> Result<()> {
        let env = hook.env_path().expect("Node must have env path");
        fs_err::create_dir_all(env)?;

        let store = Store::from_settings()?;
        let node_dir = store.tools_path(ToolBucket::Node);

        let installer = NodeInstaller::new(node_dir);
        let node = installer.install(&hook.language_version).await?;

        // TODO: Create an env
        _ = node;

        Ok(())
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        _hook: &Hook,
        _filenames: &[&String],
        _env_vars: Arc<HashMap<&'static str, String>>,
    ) -> Result<(i32, Vec<u8>)> {
        Ok((0, Vec::new()))
    }
}
