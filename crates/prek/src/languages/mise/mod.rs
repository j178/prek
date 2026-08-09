use std::ffi::{OsStr, OsString};

mod installer;
#[allow(clippy::module_inception)]
mod mise;
mod version;

pub(crate) use mise::Mise;
pub(crate) use version::MiseRequest;

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
