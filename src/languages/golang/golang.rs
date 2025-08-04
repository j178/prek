use std::collections::HashMap;
use std::sync::Arc;

use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::languages::golang::GoRequest;
use crate::languages::golang::installer::GoInstaller;
use crate::languages::version::LanguageRequest;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Golang;

impl LanguageImpl for Golang {
    async fn install(&self, hook: Arc<Hook>, store: &Store) -> anyhow::Result<InstalledHook> {
        let go_dir = store.tools_path(crate::store::ToolBucket::Golang);
        let installer = GoInstaller::new(go_dir);

        let version = match &hook.language_request {
            LanguageRequest::Any => &GoRequest::Any,
            LanguageRequest::Golang(version) => version,
            _ => unreachable!(),
        };
        let go = installer.install(version).await?;

        let info = InstallInfo::new(hook.language, hook.dependencies().clone(), store);
        info.clear_env_path().await?;
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
