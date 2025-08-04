#[allow(clippy::module_inception)]
mod golang;
mod installer;
mod version;

pub(crate) use golang::Golang;
pub(crate) use version::GoRequest;
