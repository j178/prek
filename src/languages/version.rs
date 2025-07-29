use std::path::PathBuf;

use crate::config::{self, Language, LanguagePreference};
use crate::hook::InstallInfo;
use crate::languages::python;

#[derive(thiserror::Error, Debug)]
pub enum Error {
    #[error("Invalid language version: `{0}`")]
    InvalidVersion(String),
}

#[derive(Debug, Clone)]
pub struct LanguageVersion {
    pub preference: LanguagePreference,
    pub request: LanguageRequest,
}

impl LanguageVersion {
    pub fn parse(lang: Language, version: &config::LanguageVersion) -> Result<Self, Error> {
        let request = parse_language_request(lang, version.request.as_deref())?;
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

#[derive(Debug, Clone)]
pub enum LanguageRequest {
    Any,
    Range(semver::VersionReq, String),
    Path(PathBuf),
}

impl LanguageRequest {
    pub fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            LanguageRequest::Any => true,
            LanguageRequest::Range(version_req, _) => {
                version_req.matches(&install_info.language_version)
            }
            // TODO: check path
            LanguageRequest::Path(path) => &install_info.toolchain == path,
        }
    }
}

fn parse_language_request(
    language: Language,
    request: Option<&str>,
) -> Result<LanguageRequest, Error> {
    let Some(request) = request else {
        return Ok(LanguageRequest::Any);
    };

    #[allow(clippy::single_match_else)]
    match language {
        Language::Python => python::parse_version(request),
        _ => {
            // TODO: support other languages
            let req = semver::VersionReq::parse(request)
                .map_err(|_| Error::InvalidVersion(request.to_string()))?;
            Ok(LanguageRequest::Range(req, request.to_string()))
        }
    }
}
