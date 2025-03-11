use std::collections::HashMap;

use anyhow::Result;

use crate::hook::{Hook, ResolvedHook};
use crate::languages::LanguageImpl;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub struct Fail;

impl LanguageImpl for Fail {
    fn supports_dependency(&self) -> bool {
        false
    }

    async fn resolve(&self, hook: &Hook, _store: &Store) -> Result<ResolvedHook> {
        Ok(ResolvedHook::NoNeedInstall(hook.clone()))
    }

    async fn install(&self, _hook: &ResolvedHook, _store: &Store) -> Result<()> {
        Ok(())
    }

    async fn check_health(&self) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &ResolvedHook,
        filenames: &[&String],
        _env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let mut out = hook.entry.as_bytes().to_vec();
        out.extend(b"\n\n");
        for f in filenames {
            out.extend(f.as_bytes());
            out.push(b'\n');
        }
        out.push(b'\n');

        Ok((1, out))
    }
}
