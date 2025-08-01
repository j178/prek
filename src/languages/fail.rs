use std::collections::HashMap;

use crate::hook::{Hook, InstalledHook};
use crate::languages::{Error, LanguageImpl};
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Fail;

impl LanguageImpl for Fail {
    async fn install(&self, hook: &Hook, _store: &Store) -> Result<InstalledHook, Error> {
        Ok(InstalledHook::NoNeedInstall(hook.clone()))
    }

    async fn check_health(&self) -> Result<(), Error> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        _env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>), Error> {
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
