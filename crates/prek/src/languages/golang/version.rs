use std::fmt::Display;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct GoVersion(semver::Version);

impl Default for GoVersion {
    fn default() -> Self {
        GoVersion(semver::Version::new(0, 0, 0))
    }
}

impl Deref for GoVersion {
    type Target = semver::Version;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for GoVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for GoVersion {
    type Err = semver::Error;

    // TODO: go1.20.0b1, go1.20.0rc1?
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
/// `go1.20rc1` or `1.20rc1`
/// `go1.18beta1` or `1.18beta1`
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
    // TODO: support prerelease versions like `go1.20.0b1`, `go1.20rc1`
    // MajorMinorPrerelease(u64, u64, String),
}

impl Display for GoRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GoRequest::Any => write!(f, "any"),
            GoRequest::Major(major) => write!(f, "go{major}"),
            GoRequest::MajorMinor(major, minor) => write!(f, "go{major}.{minor}"),
            GoRequest::MajorMinorPatch(major, minor, patch) => {
                write!(f, "go{major}.{minor}.{patch}")
            }
            GoRequest::Path(path) => write!(f, "path: {}", path.display()),
            GoRequest::Range(_, raw) => write!(f, "{raw}"),
        }
    }
}

impl FromStr for GoRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(GoRequest::Any);
        }

        // Check if it starts with "go" - parse as specific version
        if let Some(version_part) = s.strip_prefix("go") {
            if version_part.is_empty() {
                return Ok(GoRequest::Any);
            }

            return Self::parse_version_numbers(version_part, s);
        }

        Self::parse_version_numbers(s, s)
            .or_else(|_| {
                semver::VersionReq::parse(s)
                    .map(|version_req| GoRequest::Range(version_req, s.into()))
                    .map_err(|_| Error::InvalidVersion(s.to_string()))
            })
            .or_else(|_| {
                let path = PathBuf::from(s);
                if path.exists() {
                    Ok(GoRequest::Path(path))
                } else {
                    // TODO: better error message
                    Err(Error::InvalidVersion(s.to_string()))
                }
            })
    }
}

impl GoRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, GoRequest::Any)
    }

    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> Result<GoRequest, Error> {
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(original_request.to_string()))?;

        match parts.as_slice() {
            [major] => Ok(GoRequest::Major(*major)),
            [major, minor] => Ok(GoRequest::MajorMinor(*major, *minor)),
            [major, minor, patch] => Ok(GoRequest::MajorMinorPatch(*major, *minor, *patch)),
            _ => Err(Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = &install_info.language_version;

        self.matches(
            &GoVersion(version.clone()),
            Some(install_info.toolchain.as_ref()),
        )
    }

    pub(crate) fn matches(&self, version: &GoVersion, toolchain: Option<&Path>) -> bool {
        match self {
            GoRequest::Any => true,
            GoRequest::Major(major) => version.0.major == *major,
            GoRequest::MajorMinor(major, minor) => {
                version.0.major == *major && version.0.minor == *minor
            }
            GoRequest::MajorMinorPatch(major, minor, patch) => {
                version.0.major == *major && version.0.minor == *minor && version.0.patch == *patch
            }
            // FIXME: consider resolving symlinks and normalizing paths before comparison
            GoRequest::Path(path) => toolchain.is_some_and(|t| t == path),
            GoRequest::Range(req, _) => req.matches(&version.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_go_request_from_str() {
        let cases = vec![
            ("", GoRequest::Any),
            ("go", GoRequest::Any),
            ("go1", GoRequest::Major(1)),
            ("1", GoRequest::Major(1)),
            ("go1.20", GoRequest::MajorMinor(1, 20)),
            ("1.20", GoRequest::MajorMinor(1, 20)),
            ("go1.20.3", GoRequest::MajorMinorPatch(1, 20, 3)),
            ("1.20.3", GoRequest::MajorMinorPatch(1, 20, 3)),
            (
                ">= 1.20, < 1.22",
                GoRequest::Range(
                    semver::VersionReq::parse(">= 1.20, < 1.22").unwrap(),
                    ">= 1.20, < 1.22".into(),
                ),
            ),
        ];

        for (input, expected) in cases {
            let req = GoRequest::from_str(input).unwrap();
            assert_eq!(req, expected, "Input: {input}");
        }
    }

    #[test]
    fn test_go_request_invalid() {
        let invalid_cases = vec!["go1.20.3.4", "go1.beta", "invalid_version"];
        for input in invalid_cases {
            let req = GoRequest::from_str(input);
            assert!(req.is_err(), "Input: {input}");
        }
    }

    #[test]
    fn test_go_request_matches() {
        let version = GoVersion(semver::Version::new(1, 20, 3));
        let cases = vec![
            (GoRequest::Any, true),
            (GoRequest::Major(1), true),
            (GoRequest::Major(2), false),
            (GoRequest::MajorMinor(1, 20), true),
            (GoRequest::MajorMinor(1, 21), false),
            (GoRequest::MajorMinorPatch(1, 20, 3), true),
            (GoRequest::MajorMinorPatch(1, 20, 4), false),
            (
                GoRequest::Range(
                    semver::VersionReq::parse(">= 1.19, < 1.21").unwrap(),
                    ">= 1.19, < 1.21".into(),
                ),
                true,
            ),
            (
                GoRequest::Range(
                    semver::VersionReq::parse(">= 1.21").unwrap(),
                    ">= 1.21".into(),
                ),
                false,
            ),
        ];

        for (req, expected) in cases {
            let result = req.matches(&version, None);
            assert_eq!(result, expected, "Request: {req}");
        }
    }

    #[test]
    fn test_go_request_display() {
        let cases = vec![
            (GoRequest::Any, "any"),
            (GoRequest::Major(1), "go1"),
            (GoRequest::MajorMinor(1, 20), "go1.20"),
            (GoRequest::MajorMinorPatch(1, 20, 3), "go1.20.3"),
            (
                GoRequest::Range(
                    semver::VersionReq::parse(">= 1.20, < 1.22").unwrap(),
                    ">= 1.20, < 1.22".into(),
                ),
                ">= 1.20, < 1.22",
            ),
        ];
        for (req, expected) in cases {
            let req_str = req.to_string();
            assert_eq!(req_str, expected, "Request: {req:?}");
        }
    }
}
