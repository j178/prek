use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::hook_entry::PreparedHookEntry;
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Script;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Script {
    async fn install(
        &self,
        _store: &Store,
        hook: Arc<Hook>,
        _install_cwd: &Path,
        _reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    fn prepare_hook_entry(
        &self,
        store: &Store,
        hook: &InstalledHook,
        environment: &ExecutionEnvironment,
    ) -> Result<PreparedHookEntry> {
        // For `language: script`, the `entry[0]` is a script path.
        // For remote hooks, the path is relative to the repo root.
        // For local hooks, the path is relative to the current working directory.
        let repo_path = hook.repo_path().unwrap_or(hook.work_dir());
        Ok(hook
            .entry
            .resolve_script(repo_path, environment.path(hook), hook.work_dir(), store)?)
    }
}
