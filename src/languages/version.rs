use std::str::FromStr;

use crate::config::Language;
use crate::hook::InstallInfo;
use crate::languages::golang::GoRequest;
use crate::languages::node::NodeRequest;
use crate::languages::python::PythonRequest;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("Invalid `language_version` value: `{0}`")]
    InvalidVersion(String),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum LanguageRequest {
    Any,
    Python(PythonRequest),
    Node(NodeRequest),
    Golang(GoRequest),
    // TODO: all other languages default to semver for now.
    Semver(SemverRequest),
}

impl LanguageRequest {
    pub(crate) fn is_any(&self) -> bool {
        match self {
            LanguageRequest::Any => true,
            LanguageRequest::Python(req) => req.is_any(),
            LanguageRequest::Node(req) => req.is_any(),
            LanguageRequest::Golang(req) => req.is_any(),
            LanguageRequest::Semver(_) => false,
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

        // TODO: support `system`? Does anyone use it?
        if request == "default" || request.is_empty() {
            return Ok(LanguageRequest::Any);
        }

        Ok(match lang {
            Language::Python => Self::Python(request.parse()?),
            Language::Node => Self::Node(request.parse()?),
            Language::Golang => Self::Golang(request.parse()?),
            _ => Self::Semver(request.parse()?),
        })
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            LanguageRequest::Any => true,
            LanguageRequest::Python(req) => req.satisfied_by(install_info),
            LanguageRequest::Node(req) => req.satisfied_by(install_info),
            LanguageRequest::Golang(req) => req.satisfied_by(install_info),
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
