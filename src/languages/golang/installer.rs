use std::path::PathBuf;
use crate::languages::version::{Error, LanguageRequest};

/// `language_version` field of golang can be one of the following:
/// `default`
/// `system`
/// `go`
/// `go1.20` or `1.20`
/// `go1.20.3` or `1.20.3`
/// `go1.20.0b1` or `1.20.0b1`
/// `go1.20.0rc1` or `1.20.0rc1`
/// `>= 1.20, < 1.22`
/// `local/path/to/go`
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum GolangRequest{
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Path(PathBuf),
    Range(semver::VersionReq, String),
}

impl GolangRequest {
    pub fn parse(request: &str) -> Result<LanguageRequest, Error> {
        if request.is_empty() {
            return Ok(LanguageRequest::Any);
        }

        // Check if it starts with "go" - parse as specific version
        let request = if let Some(version_part) = request.strip_prefix("go") {
            if version_part.is_empty() {
                return Ok(GolangRequest::Major(1)); // Default to major version 1
            }
            Self::parse_version_numbers(version_part, request)
        } else {
            Self::parse_version_numbers(request, request)
                .or_else(|_| {
                    // Try to parse as a VersionReq (like ">= 1.20" or ">=1.20, <1.22")
                    semver::VersionReq::parse(request)
                        .map(|version_req| GolangRequest::Range(version_req, request.into()))
                        .map_err(|_| semver::Error::InvalidVersion(request.to_string()))
                })
                .or_else(|_| {
                    // If it doesn't match any known format, treat it as a path
                    let path = PathBuf::from(request);
                    if path.exists() {
                        Ok(GolangRequest::Path(path))
                    } else {
                        Err(semver::Error::InvalidVersion(request.to_string()))
                    }
                })
        };

        request
    }

    fn parse_version_numbers(version_part: &str, original_request: &str) -> Result<GolangRequest, semver::Error> {
        let parts: Vec<&str> = version_part.split('.').collect();
        match parts.len() {
            1 => parts[0].parse::<u64>()
                .map(GolangRequest::Major)
                .map_err(|_| semver::Error::InvalidVersion(original_request.to_string())),
            2 => {
                let major = parts[0].parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                let minor = parts[1].parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                Ok(GolangRequest::MajorMinor(major, minor))
            }
            3 => {
                let major = parts[0].parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                let minor = parts[1].parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                let patch = parts[2].parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                Ok(GolangRequest::MajorMinorPatch(major, minor, patch))
            }
            _ => Err(semver::Error::InvalidVersion(original_request.to_string())),
        }
    }
}
