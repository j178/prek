mod installer;
#[allow(clippy::module_inception)]
mod rust;
mod rustup;
mod version;

pub(crate) use rust::Rust;
pub(crate) use version::RustRequest;
