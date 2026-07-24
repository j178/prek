use std::str::FromStr;

use crate::config::Language;
use crate::hook::InstallInfo;
use crate::languages::bun::BunRequest;
use crate::languages::deno::DenoRequest;
use crate::languages::dotnet::DotnetRequest;
use crate::languages::golang::GoRequest;
use crate::languages::node::NodeRequest;
use crate::languages::python::PythonRequest;
use crate::languages::ruby::RubyRequest;
use crate::languages::rust::RustRequest;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("Invalid `language_version` value: `{0}`")]
    InvalidVersion(String),
}

/// A version constraint together with the policy for acquiring a matching toolchain.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct LanguageRequest {
    version: VersionRequest,
    allows_download: bool,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum VersionRequest {
    Any,
    Bun(BunRequest),
    Dotnet(DotnetRequest),
    Deno(DenoRequest),
    Golang(GoRequest),
    Ruby(RubyRequest),
    Node(NodeRequest),
    Python(PythonRequest),
    Rust(RustRequest),
    // TODO: all other languages default to semver for now.
    Semver(SemverRequest),
}

pub(crate) trait LanguageVersionRequest {
    fn from_version_request(request: &VersionRequest) -> &Self;
}

macro_rules! impl_language_version_request {
    ($request:ty, $variant:ident) => {
        impl LanguageVersionRequest for $request {
            fn from_version_request(request: &VersionRequest) -> &Self {
                match request {
                    VersionRequest::Any => &Self::Any,
                    VersionRequest::$variant(request) => request,
                    _ => unreachable!("language-specific version request mismatch"),
                }
            }
        }
    };
}

impl_language_version_request!(BunRequest, Bun);
impl_language_version_request!(DotnetRequest, Dotnet);
impl_language_version_request!(DenoRequest, Deno);
impl_language_version_request!(GoRequest, Golang);
impl_language_version_request!(RubyRequest, Ruby);
impl_language_version_request!(NodeRequest, Node);
impl_language_version_request!(PythonRequest, Python);
impl_language_version_request!(RustRequest, Rust);

impl Default for LanguageRequest {
    fn default() -> Self {
        Self {
            version: VersionRequest::Any,
            allows_download: true,
        }
    }
}

impl LanguageRequest {
    pub(crate) fn is_any(&self) -> bool {
        self.version.is_any()
    }

    /// Returns true if this request allows downloading a version.
    pub(crate) fn allows_download(&self) -> bool {
        self.allows_download
    }

    pub(crate) fn version_request(&self) -> &VersionRequest {
        &self.version
    }

    pub(crate) fn version<T: LanguageVersionRequest>(&self) -> &T {
        T::from_version_request(&self.version)
    }

    /// Replace only the version constraint, preserving download policy.
    pub(crate) fn set_version(&mut self, version: VersionRequest) {
        self.version = version;
    }

    pub(crate) fn parse(lang: Language, request: &str) -> Result<Self, Error> {
        // `pre-commit` support these values in `language_version`:
        // - `default`: substituted by language `get_default_version` function
        //   In `get_default_version`, if a system version is available, it will return `system`.
        //   For Python, it will find from sys.executable, `pythonX.Y`, or versions `py` can find.
        //   Otherwise, it will still return `default`.
        // - `system`: use a locally available version without downloading
        // - Python version passed down to `virtualenv`, e.g. `python`, `python3`, `python3.8`
        // - Node.js version passed down to `nodeenv`
        // - Rust version passed down to `rustup`

        Ok(Self {
            version: VersionRequest::parse(lang, request)?,
            allows_download: request != "system",
        })
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        self.version.satisfied_by(install_info)
    }
}

impl VersionRequest {
    pub(crate) fn parse(lang: Language, request: &str) -> Result<Self, Error> {
        if request == "default" || request == "system" || request.is_empty() {
            return Ok(Self::Any);
        }

        Ok(match lang {
            Language::Bun => Self::Bun(request.parse()?),
            Language::Dotnet => Self::Dotnet(request.parse()?),
            Language::Deno => Self::Deno(request.parse()?),
            Language::Golang => Self::Golang(request.parse()?),
            Language::Node => Self::Node(request.parse()?),
            Language::Python => Self::Python(request.parse()?),
            Language::Ruby => Self::Ruby(request.parse()?),
            Language::Rust => Self::Rust(request.parse()?),
            Language::Conda
            | Language::Coursier
            | Language::Dart
            | Language::Docker
            | Language::DockerImage
            | Language::Fail
            | Language::Haskell
            | Language::Julia
            | Language::Lua
            | Language::Perl
            | Language::Php
            | Language::Pygrep
            | Language::R
            | Language::Script
            | Language::Swift
            | Language::System => Self::Semver(request.parse()?),
        })
    }

    fn is_any(&self) -> bool {
        match self {
            Self::Any => true,
            Self::Bun(req) => req.is_any(),
            Self::Dotnet(req) => req.is_any(),
            Self::Deno(req) => req.is_any(),
            Self::Golang(req) => req.is_any(),
            Self::Node(req) => req.is_any(),
            Self::Python(req) => req.is_any(),
            Self::Ruby(req) => req.is_any(),
            Self::Rust(req) => req.is_any(),
            Self::Semver(_) => false,
        }
    }

    fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            Self::Any => true,
            Self::Bun(req) => req.satisfied_by(install_info),
            Self::Dotnet(req) => req.satisfied_by(install_info),
            Self::Deno(req) => req.satisfied_by(install_info),
            Self::Golang(req) => req.satisfied_by(install_info),
            Self::Node(req) => req.satisfied_by(install_info),
            Self::Python(req) => req.satisfied_by(install_info),
            Self::Ruby(req) => req.satisfied_by(install_info),
            Self::Rust(req) => req.satisfied_by(install_info),
            Self::Semver(req) => req.satisfied_by(install_info),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct SemverRequest(semver::VersionReq);

impl FromStr for SemverRequest {
    type Err = Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        semver::VersionReq::parse(request)
            .map(SemverRequest)
            .map_err(|_| Error::InvalidVersion(request.to_string()))
    }
}

impl SemverRequest {
    fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        self.0.matches(&install_info.language_version)
    }
}

pub(crate) fn try_into_u64_slice(version: &str) -> Result<Vec<u64>, std::num::ParseIntError> {
    version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
}
