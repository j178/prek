use std::fmt::Display;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct BunVersion(semver::Version);

impl Default for BunVersion {
    fn default() -> Self {
        BunVersion(semver::Version::new(0, 0, 0))
    }
}

impl Deref for BunVersion {
    type Target = semver::Version;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for BunVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for BunVersion {
    type Err = semver::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix('v').unwrap_or(s).trim();
        semver::Version::parse(s).map(BunVersion)
    }
}

/// `language_version` field of bun can be one of the following:
/// - `default`: Find system installed bun, or download the latest version.
/// - `system`: Find system installed bun, or error if not found.
/// - `bun` or `bun@latest`: Same as `default`.
/// - `x.y` or `bun@x.y`: Install the latest version with the same major and minor version.
/// - `x.y.z` or `bun@x.y.z`: Install the specific version.
/// - `^x.y.z`: Install the latest version that satisfies the semver requirement.
///   Or any other semver compatible version requirement.
/// - `local/path/to/bun`: Use bun executable at the specified path.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum BunRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Path(PathBuf),
    Range(semver::VersionReq),
}

impl FromStr for BunRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(BunRequest::Any);
        }

        // Handle "bun" or "bun@version" format
        if let Some(version_part) = s.strip_prefix("bun@") {
            if version_part.eq_ignore_ascii_case("latest") {
                return Ok(BunRequest::Any);
            }
            return Self::parse_version_numbers(version_part, s);
        }

        if s == "bun" {
            return Ok(BunRequest::Any);
        }

        Self::parse_version_numbers(s, s)
            .or_else(|_| {
                semver::VersionReq::parse(s)
                    .map(BunRequest::Range)
                    .map_err(|_| Error::InvalidVersion(s.to_string()))
            })
            .or_else(|_| {
                let path = PathBuf::from(s);
                if path.exists() {
                    Ok(BunRequest::Path(path))
                } else {
                    Err(Error::InvalidVersion(s.to_string()))
                }
            })
    }
}

impl BunRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, BunRequest::Any)
    }

    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> Result<BunRequest, Error> {
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(original_request.to_string()))?;

        match parts.as_slice() {
            [major] => Ok(BunRequest::Major(*major)),
            [major, minor] => Ok(BunRequest::MajorMinor(*major, *minor)),
            [major, minor, patch] => Ok(BunRequest::MajorMinorPatch(*major, *minor, *patch)),
            _ => Err(Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = &install_info.language_version;
        self.matches(
            &BunVersion(version.clone()),
            Some(install_info.toolchain.as_ref()),
        )
    }

    pub(crate) fn matches(&self, version: &BunVersion, toolchain: Option<&Path>) -> bool {
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
    fn test_bun_version_from_str() {
        let v: BunVersion = "1.1.0".parse().unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 1);
        assert_eq!(v.patch, 0);

        let v: BunVersion = "v1.2.3".parse().unwrap();
        assert_eq!(v.major, 1);
        assert_eq!(v.minor, 2);
        assert_eq!(v.patch, 3);
    }

    #[test]
    fn test_bun_request_from_str() {
        assert_eq!(BunRequest::from_str("bun").unwrap(), BunRequest::Any);
        assert_eq!(BunRequest::from_str("bun@latest").unwrap(), BunRequest::Any);
        assert_eq!(BunRequest::from_str("").unwrap(), BunRequest::Any);

        assert_eq!(BunRequest::from_str("1").unwrap(), BunRequest::Major(1));
        assert_eq!(BunRequest::from_str("bun@1").unwrap(), BunRequest::Major(1));

        assert_eq!(
            BunRequest::from_str("1.1").unwrap(),
            BunRequest::MajorMinor(1, 1)
        );
        assert_eq!(
            BunRequest::from_str("bun@1.1").unwrap(),
            BunRequest::MajorMinor(1, 1)
        );

        assert_eq!(
            BunRequest::from_str("1.1.0").unwrap(),
            BunRequest::MajorMinorPatch(1, 1, 0)
        );
        assert_eq!(
            BunRequest::from_str("bun@1.1.0").unwrap(),
            BunRequest::MajorMinorPatch(1, 1, 0)
        );
    }

    #[test]
    fn test_bun_request_range() {
        let req = BunRequest::from_str(">=1.0").unwrap();
        assert!(matches!(req, BunRequest::Range(_)));

        let req = BunRequest::from_str(">=1.0, <2.0").unwrap();
        assert!(matches!(req, BunRequest::Range(_)));
    }

    #[test]
    fn test_bun_request_invalid() {
        assert!(BunRequest::from_str("1.1.0.1").is_err());
        assert!(BunRequest::from_str("1.1a").is_err());
        assert!(BunRequest::from_str("invalid").is_err());
    }

    #[test]
    fn test_bun_request_matches() {
        let version = BunVersion(semver::Version::new(1, 1, 4));

        assert!(BunRequest::Any.matches(&version, None));
        assert!(BunRequest::Major(1).matches(&version, None));
        assert!(!BunRequest::Major(2).matches(&version, None));
        assert!(BunRequest::MajorMinor(1, 1).matches(&version, None));
        assert!(!BunRequest::MajorMinor(1, 2).matches(&version, None));
        assert!(BunRequest::MajorMinorPatch(1, 1, 4).matches(&version, None));
        assert!(!BunRequest::MajorMinorPatch(1, 1, 5).matches(&version, None));
    }
}
