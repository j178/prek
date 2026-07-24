use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

use serde::Deserialize;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, parse_prerelease_version, try_into_u64_slice};

#[derive(Debug, Clone, Eq, PartialEq, Deserialize)]
pub(crate) struct GoVersion(semver::Version);

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

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.strip_prefix("go").unwrap_or(s).trim();
        if let Some(version) = parse_prerelease_version(s) {
            if is_valid_go_prerelease(&version) {
                return Ok(GoVersion(version));
            }
        }
        // Fall back to plain semver parsing so exotic inputs still yield a real error. This
        // also rejects shapes `parse_prerelease_version` accepts but Go never publishes (a
        // patch alongside a prerelease, or a non-Go label), since they aren't valid semver.
        semver::Version::parse(s).map(GoVersion)
    }
}

/// Go only ever publishes patchless `beta`/`rc` prereleases (`go1.24rc1`, `go1.18beta1`); other
/// shapes `parse_prerelease_version` would otherwise accept can't be mapped to a real download.
fn is_valid_go_prerelease(version: &semver::Version) -> bool {
    if version.pre.is_empty() {
        return true;
    }
    version.patch == 0 && matches!(version.pre.as_str().split('.').next(), Some("beta" | "rc"))
}

impl GoVersion {
    /// Go-native version string (no `go` prefix), e.g. `1.24.5` or `1.24rc1`. go.dev
    /// uses this, not semver's `1.24.0-rc.1`, so downloads must go through here.
    pub(crate) fn to_go_string(&self) -> String {
        let v = &self.0;
        if !v.pre.is_empty() {
            // Go writes a prerelease without the patch: `1.24.0-rc.1` -> `1.24rc1`.
            let pre: String = v.pre.as_str().split('.').collect();
            format!("{}.{}{}", v.major, v.minor, pre)
        } else if v.patch == 0 && v.major == 1 && v.minor <= 20 {
            // Through 1.20 Go named a minor's initial release patchless (`go1.20`); 1.21 onward
            // carries the patch (`go1.21.0`, `go1.24.0`), so only collapse the older ones.
            format!("{}.{}", v.major, v.minor)
        } else {
            format!("{}.{}.{}", v.major, v.minor, v.patch)
        }
    }
}

/// `language_version` field of golang can be one of the following:
/// `default`
/// `system`
/// `go`
/// `go1.20` or `1.20`
/// `go1.20.3` or `1.20.3`
/// `go1.20rc1` or `1.20rc1`
/// `go1.18beta1` or `1.18beta1`
/// `>= 1.20, < 1.22`
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum GoRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    /// An explicit prerelease request, e.g. `go1.24rc1` or `go1.18beta1`.
    Prerelease(GoVersion, String),
    Range(semver::VersionReq, String),
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
            GoRequest::Prerelease(_, raw) | GoRequest::Range(_, raw) => write!(f, "{raw}"),
        }
    }
}

impl FromStr for GoRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(GoRequest::Any);
        }

        let (version_part, has_go_prefix) = match s.strip_prefix("go") {
            Some(rest) => (rest, true),
            None => (s, false),
        };
        if has_go_prefix && version_part.is_empty() {
            return Ok(GoRequest::Any);
        }

        if let Ok(request) = Self::parse_version_numbers(version_part, s) {
            return Ok(request);
        }

        if let Some(version) = parse_prerelease_version(version_part) {
            if !version.pre.is_empty() && is_valid_go_prerelease(&version) {
                return Ok(GoRequest::Prerelease(GoVersion(version), s.to_string()));
            }
        }

        // A range like `>= 1.20, < 1.22`, but not `go`-prefixed (`go>=1.20` is nonsense).
        if !has_go_prefix {
            if let Ok(version_req) = semver::VersionReq::parse(s) {
                return Ok(GoRequest::Range(version_req, s.to_string()));
            }
        }

        Err(Error::InvalidVersion(s.to_string()))
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

        self.matches(&GoVersion(version.clone()))
    }

    pub(crate) fn matches(&self, version: &GoVersion) -> bool {
        match self {
            GoRequest::Any => version.0.pre.is_empty(),
            GoRequest::Major(major) => version.0.pre.is_empty() && version.0.major == *major,
            GoRequest::MajorMinor(major, minor) => {
                version.0.pre.is_empty() && version.0.major == *major && version.0.minor == *minor
            }
            GoRequest::MajorMinorPatch(major, minor, patch) => {
                version.0.pre.is_empty()
                    && version.0.major == *major
                    && version.0.minor == *minor
                    && version.0.patch == *patch
            }
            GoRequest::Prerelease(requested, _) => version.0 == requested.0,
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
        let invalid_cases = vec![
            "go1.20.3.4",
            "go1.beta",
            "invalid_version",
            // Go never publishes a patch alongside a prerelease.
            "go1.24.5rc1",
            // Go only uses `beta`/`rc`, not Python-style `a`/`alpha`/`c`/`pre`/`preview`.
            "go1.24a1",
            "go1.24alpha1",
            "go1.24c1",
            "go1.24pre1",
        ];
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
            let result = req.matches(&version);
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

    #[test]
    fn test_go_request_prerelease() {
        let rc = GoRequest::from_str("go1.24rc1").unwrap();
        assert_eq!(
            rc,
            GoRequest::Prerelease(
                GoVersion(semver::Version::parse("1.24.0-rc.1").unwrap()),
                "go1.24rc1".to_string(),
            )
        );
        assert!(matches!(
            GoRequest::from_str("1.18beta1").unwrap(),
            GoRequest::Prerelease(..)
        ));

        // A prerelease request matches only that exact prerelease.
        let rc1 = GoVersion::from_str("go1.24rc1").unwrap();
        let rc2 = GoVersion::from_str("go1.24rc2").unwrap();
        let release = GoVersion::from_str("go1.24.0").unwrap();
        assert!(rc.matches(&rc1));
        assert!(!rc.matches(&rc2));
        assert!(!rc.matches(&release));

        // Neither a stable request nor `Any` (the default) selects a prerelease.
        let stable = GoRequest::from_str("go1.24").unwrap();
        assert!(!stable.matches(&rc1));
        assert!(stable.matches(&release));
        assert!(!GoRequest::Any.matches(&rc1));
        assert!(GoRequest::Any.matches(&release));
    }

    #[test]
    fn test_go_version_to_go_string() {
        for input in [
            "go1.24rc1",
            "1.18beta1",
            "go1.24.5",
            "1.20.3",
            "go1.20",
            "go1.24.0",
        ] {
            let expected = input.strip_prefix("go").unwrap_or(input);
            assert_eq!(GoVersion::from_str(input).unwrap().to_go_string(), expected);
        }
        // Go has no `go1.20.0` (<=1.20 initial releases are patchless), but `go1.24.0` is real.
        assert_eq!(
            GoVersion::from_str("go1.20.0").unwrap().to_go_string(),
            "1.20"
        );
    }

    #[test]
    fn test_go_version_prerelease_parsing() {
        assert_eq!(
            *GoVersion::from_str("go1.24rc1").unwrap(),
            semver::Version::parse("1.24.0-rc.1").unwrap()
        );
        // Numeric (not lexical) ordering, and prerelease < release.
        assert!(
            *GoVersion::from_str("1.24rc9").unwrap() < *GoVersion::from_str("1.24rc10").unwrap()
        );
        assert!(*GoVersion::from_str("1.24rc1").unwrap() < *GoVersion::from_str("1.24.0").unwrap());
    }

    #[test]
    fn go_version_rejects_non_go_prerelease_shapes() {
        // A patch alongside a prerelease, and Python-style labels, are not real Go versions.
        for input in ["1.24.5rc1", "1.24a1", "1.24alpha1", "1.24c1", "1.24pre1"] {
            assert!(GoVersion::from_str(input).is_err(), "Input: {input}");
        }
    }
}
