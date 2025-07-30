use crate::config::{self, Language, LanguagePreference};
use crate::hook::InstallInfo;
use crate::languages::python::PythonRequest;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Invalid language version: `{0}`")]
    InvalidVersion(String),
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum LanguageRequest {
    Any,
    Semver(SemverRequest),
    Python(PythonRequest),
}

impl LanguageRequest {
    fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            LanguageRequest::Any => true,
            LanguageRequest::Semver(req) => req.satisfied_by(install_info),
            LanguageRequest::Python(req) => req.satisfied_by(install_info),
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

#[derive(Debug, Clone)]
pub struct LanguageVersion {
    pub preference: LanguagePreference,
    pub request: LanguageRequest,
}

impl LanguageVersion {
    pub fn parse(lang: Language, version: &config::LanguageVersion) -> Result<Self, Error> {
        let Some(ref request) = version.request else {
            return Ok(Self {
                preference: version.preference,
                request: LanguageRequest::Any,
            });
        };

        #[allow(clippy::single_match_else)]
        let request = match lang {
            Language::Python => PythonRequest::parse(request)?,
            _ => {
                // TODO: support other languages
                SemverRequest::parse(request)?
            }
        };

        Ok(Self {
            preference: version.preference,
            request,
        })
    }

    pub fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        // TODO: check preference?
        self.request.satisfied_by(install_info)
    }

    pub fn allow_system(&self) -> bool {
        matches!(
            self.preference,
            LanguagePreference::Managed | LanguagePreference::OnlySystem
        )
    }

    pub fn allow_managed(&self) -> bool {
        matches!(
            self.preference,
            LanguagePreference::Managed | LanguagePreference::OnlyManaged
        )
    }
}
