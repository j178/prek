use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::hook::{Hook, InstalledHook};
use crate::languages::LanguageImpl;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Fail;

impl LanguageImpl for Fail {
    async fn install(&self, hook: Arc<Hook>, _store: &Store) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        _env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let mut out = shlex::try_join(hook.entry.parsed()?.iter().map(std::ops::Deref::deref))
            .expect("Failed to join `entry` as command")
            .into_bytes();
        out.extend(b"\n\n");
        for f in filenames {
            out.extend(f.as_bytes());
            out.push(b'\n');
        }
        out.push(b'\n');

        Ok((1, out))
    }
}
