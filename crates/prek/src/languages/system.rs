use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageBackend;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct System;

#[async_trait::async_trait(?Send)]
impl LanguageBackend for System {
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
}
