//! Implement `-p <python_spec>` argument parser of `virutualenv` from
//! <https://github.com/pypa/virtualenv/blob/216dc9f3592aa1f3345290702f0e7ba3432af3ce/src/virtualenv/discovery/py_spec.py>

use std::path::PathBuf;

use crate::languages::version;
use crate::languages::version::LanguageRequest;

// TODO: parse Python style version like `3.8b1`, `3.8rc2`, `python3.8t`, `python3.8-64` into semver.
pub fn parse_version(request: &str) -> Result<LanguageRequest, version::Error> {
    if let Some(request) = request.strip_prefix("python") {
        if request.is_empty() {
            return Ok(LanguageRequest::Any);
        }
        let mut parts = request.split('.').collect::<Vec<_>>();
        if parts.len() > 3 {
            return Err(version::Error::InvalidVersion(request.to_string()));
        }
        // Fill missing parts with `0`
        while parts.len() < 3 {
            parts.push("0");
        }
        let version_str = parts.join(".");
        let req = semver::VersionReq::parse(&version_str)
            .map_err(|_| version::Error::InvalidVersion(request.to_string()))?;
        return Ok(LanguageRequest::Range(req, request.into()));
    }

    if request.is_empty() {
        return Ok(LanguageRequest::Any);
    }
    if let Ok(version_req) = semver::VersionReq::parse(request) {
        Ok(LanguageRequest::Range(version_req, request.into()))
    } else {
        Ok(LanguageRequest::Path(PathBuf::from(request)))
    }
}
