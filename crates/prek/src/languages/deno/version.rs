use std::fmt::Display;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Clone, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct DenoVersion(semver::Version);

impl Default for DenoVersion {
    fn default() -> Self {
        DenoVersion(semver::Version::new(0, 0, 0))
    }
}

impl Deref for DenoVersion {
    type Target = semver::Version;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for DenoVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for DenoVersion {
    type Err = semver::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix('v').unwrap_or(s).trim();
        semver::Version::parse(s).map(DenoVersion)
    }
}

/// `language_version` field of deno can be one of the following:
/// - `default`: Find system installed deno, or download the latest version.
/// - `system`: Find system installed deno, or error if not found.
/// - `deno` or `deno@latest`: Same as `default`.
/// - `x.y` or `deno@x.y`: Install the latest version with the same major and minor version.
/// - `x.y.z` or `deno@x.y.z`: Install the specific version.
/// - `^x.y.z`: Install the latest version that satisfies the semver requirement.
///   Or any other semver compatible version requirement.
/// - `local/path/to/deno`: Use deno executable at the specified path.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum DenoRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Path(PathBuf),
    Range(semver::VersionReq),
}

impl FromStr for DenoRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(DenoRequest::Any);
        }

        // Handle "deno" or "deno@version" format
        if let Some(version_part) = s.strip_prefix("deno@") {
            if version_part.eq_ignore_ascii_case("latest") {
                return Ok(DenoRequest::Any);
            }
            return Self::parse_version_numbers(version_part, s);
        }

        if s == "deno" {
            return Ok(DenoRequest::Any);
        }

        Self::parse_version_numbers(s, s)
            .or_else(|_| {
                semver::VersionReq::parse(s)
                    .map(DenoRequest::Range)
                    .map_err(|_| Error::InvalidVersion(s.to_string()))
            })
            .or_else(|_| {
                let path = PathBuf::from(s);
                if path.exists() {
                    Ok(DenoRequest::Path(path))
                } else {
                    Err(Error::InvalidVersion(s.to_string()))
                }
            })
    }
}

impl DenoRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, DenoRequest::Any)
    }

    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> Result<DenoRequest, Error> {
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(original_request.to_string()))?;

        match parts.as_slice() {
            [major] => Ok(DenoRequest::Major(*major)),
            [major, minor] => Ok(DenoRequest::MajorMinor(*major, *minor)),
            [major, minor, patch] => Ok(DenoRequest::MajorMinorPatch(*major, *minor, *patch)),
            _ => Err(Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = &install_info.language_version;
        self.matches(
            &DenoVersion(version.clone()),
            Some(install_info.toolchain.as_ref()),
        )
    }

    pub(crate) fn matches(&self, version: &DenoVersion, toolchain: Option<&Path>) -> bool {
        match self {
            Self::Any => true,
            Self::Major(major) => version.major == *major,
            Self::MajorMinor(major, minor) => version.major == *major && version.minor == *minor,
            Self::MajorMinorPatch(major, minor, patch) => {
                version.major == *major && version.minor == *minor && version.patch == *patch
            }
            Self::Path(path) => toolchain.is_some_and(|toolchain_path| toolchain_path == path),
            Self::Range(req) => req.matches(version),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_deno_version_from_str() {
        let v: DenoVersion = "2.1.0".parse().unwrap();
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);

        let v: DenoVersion = "v2.1.3".parse().unwrap();
        assert_eq!(v.major, 2);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_deno_request_from_str() {
        assert_eq!(DenoRequest::from_str("deno").unwrap(), DenoRequest::Any);
        assert_eq!(
            DenoRequest::from_str("deno@latest").unwrap(),
            DenoRequest::Any
        );
        assert_eq!(DenoRequest::from_str("").unwrap(), DenoRequest::Any);

        assert_eq!(DenoRequest::from_str("2").unwrap(), DenoRequest::Major(2));
        assert_eq!(
            DenoRequest::from_str("deno@2").unwrap(),
            DenoRequest::Major(2)
        );

        assert_eq!(
            DenoRequest::from_str("2.1").unwrap(),
            DenoRequest::MajorMinor(2, 1)
        );
        assert_eq!(
            DenoRequest::from_str("deno@2.1").unwrap(),
            DenoRequest::MajorMinor(2, 1)
        );

        assert_eq!(
            DenoRequest::from_str("2.1.0").unwrap(),
            DenoRequest::MajorMinorPatch(2, 1, 0)
        );
        assert_eq!(
            DenoRequest::from_str("deno@2.1.0").unwrap(),
            DenoRequest::MajorMinorPatch(2, 1, 0)
        );
    }

    #[test]
    fn test_deno_request_range() {
        let req = DenoRequest::from_str(">=2.0").unwrap();
        assert!(matches!(req, DenoRequest::Range(_)));

        let req = DenoRequest::from_str(">=2.0, <3.0").unwrap();
        assert!(matches!(req, DenoRequest::Range(_)));
    }

    #[test]
    fn test_deno_request_invalid() {
        assert!(DenoRequest::from_str("2.1.0.1").is_err());
        assert!(DenoRequest::from_str("2.1a").is_err());
        assert!(DenoRequest::from_str("invalid").is_err());
    }

    #[test]
    fn test_deno_request_matches() {
        let version = DenoVersion(semver::Version::new(2, 1, 4));

        assert!(DenoRequest::Any.matches(&version, None));
        assert!(DenoRequest::Major(2).matches(&version, None));
        assert!(!DenoRequest::Major(1).matches(&version, None));
        assert!(DenoRequest::MajorMinor(2, 1).matches(&version, None));
        assert!(!DenoRequest::MajorMinor(2, 2).matches(&version, None));
        assert!(DenoRequest::MajorMinorPatch(2, 1, 4).matches(&version, None));
        assert!(!DenoRequest::MajorMinorPatch(2, 1, 5).matches(&version, None));
    }
}
