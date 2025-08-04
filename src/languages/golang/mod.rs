#[allow(clippy::module_inception)]
mod golang;
mod installer;

pub(crate) use golang::Golang;
pub(crate) use installer::GolangRequest;
