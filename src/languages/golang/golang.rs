use std::collections::HashMap;
use std::sync::Arc;

use crate::hook::{Hook, InstalledHook};
use crate::languages::LanguageImpl;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Golang;

impl LanguageImpl for Golang {
    async fn install(&self, hook: Arc<Hook>, store: &Store) -> anyhow::Result<InstalledHook> {
        todo!()
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
        store: &Store,
    ) -> anyhow::Result<(i32, Vec<u8>)> {
        todo!()
    }
}
