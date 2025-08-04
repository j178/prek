use std::collections::HashMap;
use std::ops::Deref;
use std::sync::Arc;

use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::{create_symlink_or_copy, LanguageImpl};
use crate::languages::golang::GoRequest;
use crate::languages::golang::installer::GoInstaller;
use crate::languages::version::LanguageRequest;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Golang;

impl LanguageImpl for Golang {
    async fn install(&self, hook: Arc<Hook>, store: &Store) -> anyhow::Result<InstalledHook> {
        // 1. Install Go
        let go_dir = store.tools_path(crate::store::ToolBucket::Golang);
        let installer = GoInstaller::new(go_dir);

        let version = match &hook.language_request {
            LanguageRequest::Any => &GoRequest::Any,
            LanguageRequest::Golang(version) => version,
            _ => unreachable!(),
        };
        let go = installer.install(version).await?;

        let mut info = InstallInfo::new(hook.language, hook.dependencies().clone(), &store.hooks_dir());
        info.with_toolchain(go.bin().to_path_buf())
            .with_language_version(go.version().deref().clone());

        // 2. Create environment
        fs_err::tokio::create_dir_all(&info.env_path).await?;
        create_symlink_or_copy(go.bin(), &info.env_path).await?;

        // 3. Install dependencies
        go.cmd("go install")
            .arg("");


        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
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
