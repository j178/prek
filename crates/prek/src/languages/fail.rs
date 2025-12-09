use std::io::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;

use crate::cli::reporter::HookInstallReporter;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::languages::LanguageImpl;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Fail;

impl LanguageImpl for Fail {
    async fn install(
        &self,
        hook: Arc<Hook>,
        _store: &Store,
        _reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let mut out = Vec::new();
        writeln!(out, "{}\n", hook.entry.raw())?;
        for f in filenames {
            out.extend(f.to_string_lossy().as_bytes());
            out.push(b'\n');
        }
        out.push(b'\n');

        Ok((1, out))
    }
}
