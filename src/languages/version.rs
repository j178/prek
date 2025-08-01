use crate::config::Language;
use crate::hook::InstallInfo;
use crate::languages::node::NodeRequest;
use crate::languages::python::PythonRequest;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Invalid language version: `{0}`")]
    InvalidVersion(String),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LanguageRequest {
    Any,
    Python(PythonRequest),
    Node(NodeRequest),
    // TODO: all other languages default to semver for now.
    Semver(SemverRequest),
}

impl LanguageRequest {
    pub fn parse(lang: Language, request: &str) -> Result<Self, Error> {
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

        match lang {
            Language::Python => PythonRequest::parse(request),
            Language::Node => NodeRequest::parse(request),
            _ => SemverRequest::parse(request),
        }
    }

    pub fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            LanguageRequest::Any => true,
            LanguageRequest::Node(req) => req.satisfied_by(install_info),
            LanguageRequest::Python(req) => req.satisfied_by(install_info),
            LanguageRequest::Semver(req) => req.satisfied_by(install_info),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub struct SemverRequest(semver::VersionReq);

impl SemverRequest {
    fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        self.0.matches(&install_info.language_version)
    }
}

impl SemverRequest {
    pub fn parse(request: &str) -> Result<LanguageRequest, Error> {
        let version_req = semver::VersionReq::parse(request)
            .map_err(|_| Error::InvalidVersion(request.to_string()))?;
        Ok(LanguageRequest::Semver(SemverRequest(version_req)))
    }
}

pub(crate) fn try_into_u8_slice(version: &str) -> Result<Vec<u8>, std::num::ParseIntError> {
    version
        .split('.')
        .map(str::parse::<u8>)
        .collect::<Result<Vec<_>, _>>()
}
