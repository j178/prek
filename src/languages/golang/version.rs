use crate::languages::version::{Error, LanguageRequest};
use serde::Deserialize;
use std::path::PathBuf;
use std::str::FromStr;

pub(crate) struct GoVersion(semver::Version);

impl<'de> Deserialize<'de> for GoVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct _Version {
            version: String,
        }

        let v = _Version::deserialize(deserializer)?;
        // TODO: go1.20.0b1, go1.20.0rc1?
        let version_str = v.version.strip_prefix("go").unwrap_or(&v.version).trim();
        semver::Version::parse(&version_str)
            .map(GoVersion)
            .map_err(serde::de::Error::custom)
    }
}

impl FromStr for GoVersion {
    type Err = semver::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix("go").unwrap_or(s).trim();
        semver::Version::parse(s).map(GoVersion)
    }
}

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
pub(crate) enum GoRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Path(PathBuf),
    Range(semver::VersionReq, String),
}

impl GoRequest {

    pub(crate) fn matches(&self, version: &GoVersion) -> bool {
        match self {
            GoRequest::Any => true,
            GoRequest::Major(major) => version.0.major == *major,
            GoRequest::MajorMinor(major, minor) => {
                version.0.major == *major && version.0.minor == *minor
            }
            GoRequest::MajorMinorPatch(major, minor, patch) => {
                version.0.major == *major && version.0.minor == *minor && version.0.patch == *patch
            }
            GoRequest::Path(path) => path.exists(),
            GoRequest::Range(req, _) => req.matches(&version.0),
        }
    }

    fn parse_version_numbers(
        version_part: &str,
        original_request: &str,
    ) -> Result<GoRequest, semver::Error> {
        let parts: Vec<&str> = version_part.split('.').collect();
        match parts.len() {
            1 => parts[0]
                .parse::<u64>()
                .map(GoRequest::Major)
                .map_err(|_| semver::Error::InvalidVersion(original_request.to_string())),
            2 => {
                let major = parts[0]
                    .parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                let minor = parts[1]
                    .parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                Ok(GoRequest::MajorMinor(major, minor))
            }
            3 => {
                let major = parts[0]
                    .parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                let minor = parts[1]
                    .parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                let patch = parts[2]
                    .parse::<u64>()
                    .map_err(|_| semver::Error::InvalidVersion(original_request.to_string()))?;
                Ok(GoRequest::MajorMinorPatch(major, minor, patch))
            }
            _ => Err(semver::Error::InvalidVersion(original_request.to_string())),
        }
    }
}
