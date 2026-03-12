#[allow(clippy::module_inception)]
mod dotnet;
mod version;

pub(crate) use dotnet::Dotnet;
pub(crate) use version::DotnetRequest;
