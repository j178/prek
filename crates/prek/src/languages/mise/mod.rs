use std::ffi::{OsStr, OsString};
use std::path::Path;

use anyhow::{Context, Result};

mod installer;
#[allow(clippy::module_inception)]
mod mise;

pub(crate) use mise::Mise;

fn mise_ceiling(cwd: &Path) -> Result<OsString> {
    std::env::join_paths([cwd])
        .context("Failed to isolate mise from working directory configuration")
}

fn is_mise_var(key: impl AsRef<OsStr>) -> bool {
    let key = key.as_ref().to_string_lossy();
    #[cfg(windows)]
    let key = key.to_ascii_uppercase();
    key.starts_with("MISE_") || key.starts_with("__MISE_")
}

fn inherited_mise_vars() -> impl Iterator<Item = OsString> {
    std::env::vars_os()
        .map(|(key, _)| key)
        .filter(|key| is_mise_var(key))
}
