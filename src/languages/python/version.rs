//! Implement `-p <python_spec>` argument parser of `virutualenv` from
//! <https://github.com/pypa/virtualenv/blob/216dc9f3592aa1f3345290702f0e7ba3432af3ce/src/virtualenv/discovery/py_spec.py>

use std::path::{Path, PathBuf};

use crate::languages::version;
use crate::languages::version::LanguageRequest;

#[derive(Debug, Clone)]
pub enum PythonRequest {
    Major(u8),
    MajorMinor(u8, u8),
    MajorMinorPatch(u8, u8, u8),
    Path(PathBuf),
    Range(semver::VersionReq, String),
}

impl PythonRequest {
    // TODO: python3 => compare major, python3.13 => compare major and minor,
    // TODO: parse Python style version like `3.8b1`, `3.8rc2`, `python3.8t`, `python3.8-64` into semver.
    pub fn parse(request: &str) -> Result<LanguageRequest, version::Error> {
        if let Some(request) = request.strip_prefix("python") {
            if request.is_empty() {
                return Ok(LanguageRequest::Any);
            }
            let parts = request.split('.').collect::<Vec<_>>();
            if parts.len() > 3 {
                return Err(version::Error::InvalidVersion(request.to_string()));
            }
            return match parts[..] {
                [major] => {
                    let major = major
                        .parse::<u8>()
                        .map_err(|_| version::Error::InvalidVersion(request.to_string()))?;
                    Ok(LanguageRequest::Python(PythonRequest::Major(major)))
                }
                [major, minor] => {
                    let major = major
                        .parse::<u8>()
                        .map_err(|_| version::Error::InvalidVersion(request.to_string()))?;
                    let minor = minor
                        .parse::<u8>()
                        .map_err(|_| version::Error::InvalidVersion(request.to_string()))?;
                    Ok(LanguageRequest::Python(PythonRequest::MajorMinor(
                        major, minor,
                    )))
                }
                [major, minor, patch] => {
                    let major = major
                        .parse::<u8>()
                        .map_err(|_| version::Error::InvalidVersion(request.to_string()))?;
                    let minor = minor
                        .parse::<u8>()
                        .map_err(|_| version::Error::InvalidVersion(request.to_string()))?;
                    let patch = patch
                        .parse::<u8>()
                        .map_err(|_| version::Error::InvalidVersion(request.to_string()))?;
                    Ok(LanguageRequest::Python(PythonRequest::MajorMinorPatch(
                        major, minor, patch,
                    )))
                }
                _ => Err(version::Error::InvalidVersion(request.to_string())),
            };
        }

        if request.is_empty() {
            return Ok(LanguageRequest::Any);
        }
        if Path::new(request).exists() {
            return Ok(LanguageRequest::Python(PythonRequest::Path(PathBuf::from(
                request,
            ))));
        }

        if let Ok(version_req) = semver::VersionReq::parse(request) {
            Ok(LanguageRequest::Python(PythonRequest::Range(
                version_req,
                request.into(),
            )))
        } else {
            Err(version::Error::InvalidVersion(request.to_string()))
        }
    }
}

impl PythonRequest {
    pub(crate) fn satisfied_by(&self, install_info: &crate::hook::InstallInfo) -> bool {
        match self {
            PythonRequest::Major(major) => install_info.language_version.major == u64::from(*major),
            PythonRequest::MajorMinor(major, minor) => {
                install_info.language_version.major == u64::from(*major)
                    && install_info.language_version.minor == u64::from(*minor)
            }
            PythonRequest::MajorMinorPatch(major, minor, patch) => {
                install_info.language_version.major == u64::from(*major)
                    && install_info.language_version.minor == u64::from(*minor)
                    && install_info.language_version.patch == u64::from(*patch)
            }
            PythonRequest::Path(path) => path == &install_info.toolchain,
            PythonRequest::Range(req, _) => req.matches(&install_info.language_version),
        }
    }
}
