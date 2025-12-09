#[allow(clippy::module_inception)]
mod golang;
mod gomod;
mod installer;
mod version;

pub(crate) use golang::Golang;
pub(crate) use gomod::extract_go_mod_metadata;
pub(crate) use version::GoRequest;
