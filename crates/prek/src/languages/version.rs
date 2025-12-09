use std::str::FromStr;

use crate::config::Language;
use crate::hook::InstallInfo;
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum LanguageRequest {
    Any { system_only: bool },
    Golang(GoRequest),
    Ruby(RubyRequest),
    Node(NodeRequest),
    Python(PythonRequest),
    Rust(RustRequest),
    // TODO: all other languages default to semver for now.
    Semver(SemverRequest),
}

impl LanguageRequest {
    pub(crate) fn is_any(&self) -> bool {
        match self {
            LanguageRequest::Any { .. } => true,
            LanguageRequest::Golang(req) => req.is_any(),
            LanguageRequest::Node(req) => req.is_any(),
            LanguageRequest::Python(req) => req.is_any(),
            LanguageRequest::Ruby(req) => req.is_any(),
            LanguageRequest::Rust(req) => req.is_any(),
            LanguageRequest::Semver(_) => false,
        }
    }

    /// Returns true if this request allows downloading a version.
    ///
    /// Currently, only `system` disallows downloading. In the future,
    /// we may add more specific version requests that also disallow downloading.
    /// For example `language_version: 3.12; system_only`.
    pub(crate) fn allows_download(&self) -> bool {
        match self {
            LanguageRequest::Any { system_only } => !system_only,
            _ => true,
        }
    }

    pub(crate) fn parse(lang: Language, request: &str) -> Result<Self, Error> {
        // `pre-commit` support these values in `language_version`:
        // - `default`: substituted by language `get_default_version` function
        //   In `get_default_version`, if a system version is available, it will return `system`.
        //   For Python, it will find from sys.executable, `pythonX.Y`, or versions `py` can find.
        //   Otherwise, it will still return `default`.
        // - `system`: use current system installed version
        // - Python version passed down to `virtualenv`, e.g. `python`, `python3`, `python3.8`
        // - Node.js version passed down to `nodeenv`
        // - Rust version passed down to `rustup`

        if request == "default" || request.is_empty() {
            return Ok(LanguageRequest::Any { system_only: false });
        }
        if request == "system" {
            return Ok(LanguageRequest::Any { system_only: true });
        }

        Ok(match lang {
            Language::Golang => Self::Golang(request.parse()?),
            Language::Node => Self::Node(request.parse()?),
            Language::Python => Self::Python(request.parse()?),
            Language::Ruby => Self::Ruby(request.parse()?),
            Language::Rust => Self::Rust(request.parse()?),
            _ => Self::Semver(request.parse()?),
        })
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            LanguageRequest::Any { .. } => true,
            LanguageRequest::Golang(req) => req.satisfied_by(install_info),
            LanguageRequest::Node(req) => req.satisfied_by(install_info),
            LanguageRequest::Python(req) => req.satisfied_by(install_info),
            LanguageRequest::Ruby(req) => req.satisfied_by(install_info),
            LanguageRequest::Rust(req) => req.satisfied_by(install_info),
            LanguageRequest::Semver(req) => req.satisfied_by(install_info),
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
