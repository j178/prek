#[allow(clippy::module_inception)]
mod dotnet;
pub(crate) mod installer;
mod version;

pub(crate) use dotnet::Dotnet;
pub(crate) use version::DotnetRequest;
