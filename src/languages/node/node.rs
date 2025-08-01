use std::collections::HashMap;

use crate::hook::Hook;
use crate::hook::InstalledHook;
use crate::languages::{Error, LanguageImpl};
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub struct Node;

impl LanguageImpl for Node {
    async fn install(&self, _hook: &Hook, _store: &Store) -> Result<InstalledHook, Error> {
        // let env = hook.env_path().expect("Node must have env path");
        // fs_err::create_dir_all(env)?;
        //
        // let node_dir = store.tools_path(ToolBucket::Node);
        //
        // let installer = NodeInstaller::new(node_dir);
        // let node = installer.install(&hook.language_request).await?;
        //
        // // TODO: Create an env
        // _ = node;
        //
        // Ok(())
        todo!()
    }

    async fn check_health(&self) -> Result<(), Error> {
        todo!()
    }

    async fn run(
        &self,
        _hook: &InstalledHook,
        _filenames: &[&String],
        _env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>), Error> {
        Ok((0, Vec::new()))
    }
}
