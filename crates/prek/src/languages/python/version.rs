//! Implement `-p <python_spec>` argument parser of `virtualenv` from
//! <https://github.com/pypa/virtualenv/blob/216dc9f3592aa1f3345290702f0e7ba3432af3ce/src/virtualenv/discovery/py_spec.py>
use std::str::FromStr;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, parse_prerelease_version, try_into_u64_slice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Prerelease(semver::Version, String),
    Range(semver::VersionReq, String),
}

/// Represents a request for a specific Python version.
/// example formats:
/// - `python`
/// - `python3`
/// - `python3.12`
/// - `python3.13.2`
/// - `python311`
/// - `3`
/// - `3.12`
/// - `3.12.3`
/// - `>=3.12`
/// - `>=3.8, <3.12`
/// - `3.13.0rc1`, `3.14.0a1`
// TODO: support `python3.8t` (free-threaded), `python3.8-64`, `pypy3.8`.
impl FromStr for PythonRequest {
    type Err = Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        if request.is_empty() {
            return Ok(Self::Any);
        }

        let (version_part, has_python_prefix) = match request.strip_prefix("python") {
            Some(rest) => (rest, true),
            None => (request, false),
        };
        if has_python_prefix && version_part.is_empty() {
            return Ok(Self::Any);
        }

        if let Ok(req) = Self::parse_version_numbers(version_part, request) {
            return Ok(req);
        }

        if let Some(version) = parse_prerelease_version(version_part) {
            if !version.pre.is_empty() {
                let version = normalize_prerelease_label(version);
                return Ok(PythonRequest::Prerelease(version, version_part.to_string()));
            }
        }

        // A range like `>=3.8, <3.12`, but not `python`-prefixed.
        if !has_python_prefix {
            if let Ok(version_req) = semver::VersionReq::parse(request) {
                return Ok(PythonRequest::Range(version_req, request.into()));
            }
        }

        Err(Error::InvalidVersion(request.to_string()))
    }
}

impl PythonRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, PythonRequest::Any)
    }

    /// Parse version numbers into appropriate `PythonRequest` variants
    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> Result<PythonRequest, Error> {
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(original_request.to_string()))?;
        let parts = split_wheel_tag_version(parts);

        match parts[..] {
            [major] => Ok(PythonRequest::Major(major)),
            [major, minor] => Ok(PythonRequest::MajorMinor(major, minor)),
            [major, minor, patch] => Ok(PythonRequest::MajorMinorPatch(major, minor, patch)),
            _ => Err(Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = &install_info.language_version;
        match self {
            // Stable requests never match a prerelease interpreter (`3.13.0` != `3.13.0rc1`).
            PythonRequest::Any => version.pre.is_empty(),
            PythonRequest::Major(major) => version.pre.is_empty() && version.major == *major,
            PythonRequest::MajorMinor(major, minor) => {
                version.pre.is_empty() && version.major == *major && version.minor == *minor
            }
            PythonRequest::MajorMinorPatch(major, minor, patch) => {
                version.pre.is_empty()
                    && version.major == *major
                    && version.minor == *minor
                    && version.patch == *patch
            }
            // Match the exact prerelease (`query_python_info` records level+serial), so an
            // rc1 request is not satisfied by a final release or a different prerelease.
            PythonRequest::Prerelease(req, _) => version == req,
            PythonRequest::Range(req, _) => req.matches(version),
        }
    }
}

/// Convert a wheel tag formatted version (e.g., `38`) to multiple components (e.g., `3.8`).
///
/// The major version is always assumed to be a single digit 0-9. The minor version is all
/// the following content.
///
/// If not a wheel tag formatted version, the input is returned unchanged.
fn split_wheel_tag_version(mut version: Vec<u64>) -> Vec<u64> {
    if version.len() != 1 {
        return version;
    }

    let release = version[0].to_string();
    let mut chars = release.chars();
    let Some(major) = chars.next().and_then(|c| c.to_digit(10)) else {
        return version;
    };

    let Ok(minor) = chars.as_str().parse::<u32>() else {
        return version;
    };

    version[0] = u64::from(major);
    version.push(u64::from(minor));
    version
}

/// Normalize a PEP 440 prerelease alias to the label `query_python_info` records from
/// `sys.version_info.releaselevel` (`alpha`/`a` -> `a`, `beta`/`b` -> `b`, everything else
/// meaning "release candidate" -> `rc`), so `PythonRequest::Prerelease`'s exact-equality
/// check actually matches an installed interpreter instead of comparing distinct spellings.
fn normalize_prerelease_label(mut version: semver::Version) -> semver::Version {
    let pre = version.pre.as_str();
    let (label, number) = pre.split_once('.').unwrap_or((pre, ""));
    let canonical = match label {
        "a" | "alpha" => "a",
        "b" | "beta" => "b",
        _ => "rc", // c, rc, pre, preview
    };
    let identifier = if number.is_empty() {
        canonical.to_string()
    } else {
        format!("{canonical}.{number}")
    };
    version.pre = semver::Prerelease::new(&identifier).expect("canonical label is valid");
    version
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Language;
    use std::path::PathBuf;

    #[test]
    fn test_parse_python_request() {
        // Empty request
        assert_eq!(PythonRequest::from_str("").unwrap(), PythonRequest::Any);
        assert_eq!(
            PythonRequest::from_str("python").unwrap(),
            PythonRequest::Any
        );

        assert_eq!(
            PythonRequest::from_str("python3").unwrap(),
            PythonRequest::Major(3)
        );
        assert_eq!(
            PythonRequest::from_str("python3.12").unwrap(),
            PythonRequest::MajorMinor(3, 12)
        );
        assert_eq!(
            PythonRequest::from_str("python3.13.2").unwrap(),
            PythonRequest::MajorMinorPatch(3, 13, 2)
        );
        assert_eq!(
            PythonRequest::from_str("3").unwrap(),
            PythonRequest::Major(3)
        );
        assert_eq!(
            PythonRequest::from_str("3.12").unwrap(),
            PythonRequest::MajorMinor(3, 12)
        );
        assert_eq!(
            PythonRequest::from_str("3.12.3").unwrap(),
            PythonRequest::MajorMinorPatch(3, 12, 3)
        );
        assert_eq!(
            PythonRequest::from_str("312").unwrap(),
            PythonRequest::MajorMinor(3, 12)
        );
        assert_eq!(
            PythonRequest::from_str("python312").unwrap(),
            PythonRequest::MajorMinor(3, 12)
        );

        // VersionReq
        assert_eq!(
            PythonRequest::from_str(">=3.12").unwrap(),
            PythonRequest::Range(
                semver::VersionReq::parse(">=3.12").unwrap(),
                ">=3.12".to_string()
            )
        );
        assert_eq!(
            PythonRequest::from_str(">=3.8, <3.12").unwrap(),
            PythonRequest::Range(
                semver::VersionReq::parse(">=3.8, <3.12").unwrap(),
                ">=3.8, <3.12".to_string()
            )
        );

        // Invalid versions
        assert!(PythonRequest::from_str("invalid").is_err());
        assert!(PythonRequest::from_str("3.12.3.4").is_err());
        assert!(PythonRequest::from_str("3.12.a").is_err());
        assert!(PythonRequest::from_str("3.b.1").is_err());
        assert!(PythonRequest::from_str("3..2").is_err());
        assert!(PythonRequest::from_str("a3.12").is_err());

        // PEP 440 prereleases parse to `Prerelease`, keeping the string for uv.
        assert_eq!(
            PythonRequest::from_str("3.13.0rc1").unwrap(),
            PythonRequest::Prerelease(
                semver::Version::parse("3.13.0-rc.1").unwrap(),
                "3.13.0rc1".to_string()
            )
        );
        assert!(matches!(
            PythonRequest::from_str("3.14.0a1").unwrap(),
            PythonRequest::Prerelease(..)
        ));
        assert!(matches!(
            PythonRequest::from_str("python3.13.2b2").unwrap(),
            PythonRequest::Prerelease(..)
        ));

        // Not prereleases: `t` (free-threaded) and `-64` (architecture) suffixes.
        assert!(PythonRequest::from_str("python3.13.2t1").is_err());
        assert!(PythonRequest::from_str("python3.13.2-64").is_err());
    }

    #[test]
    fn prerelease_aliases_normalize_to_the_interpreter_label() {
        // `c`, `alpha`, `beta` are PEP 440 aliases; `query_python_info` only ever emits
        // `a`/`b`/`rc`, so the alias must normalize to match or the env is never reused.
        assert_eq!(
            PythonRequest::from_str("3.13.0c1").unwrap(),
            PythonRequest::Prerelease(
                semver::Version::parse("3.13.0-rc.1").unwrap(),
                "3.13.0c1".to_string()
            )
        );
        assert_eq!(
            PythonRequest::from_str("3.14.0alpha2").unwrap(),
            PythonRequest::Prerelease(
                semver::Version::parse("3.14.0-a.2").unwrap(),
                "3.14.0alpha2".to_string()
            )
        );
        assert_eq!(
            PythonRequest::from_str("3.12.0beta3").unwrap(),
            PythonRequest::Prerelease(
                semver::Version::parse("3.12.0-b.3").unwrap(),
                "3.12.0beta3".to_string()
            )
        );
    }

    #[test]
    fn test_satisfied_by() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Python, None, Vec::new(), temp_dir.path())?;
        install_info
            .with_language_version(semver::Version::new(3, 12, 1))
            .with_toolchain(PathBuf::from("/usr/bin/python3.12"));

        assert!(PythonRequest::Any.satisfied_by(&install_info));
        assert!(PythonRequest::Major(3).satisfied_by(&install_info));
        assert!(PythonRequest::MajorMinor(3, 12).satisfied_by(&install_info));
        assert!(PythonRequest::MajorMinorPatch(3, 12, 1).satisfied_by(&install_info));
        assert!(!PythonRequest::MajorMinorPatch(3, 12, 2).satisfied_by(&install_info));

        let range_req = semver::VersionReq::parse(">=3.12").unwrap();
        assert!(PythonRequest::Range(range_req, ">=3.12".to_string()).satisfied_by(&install_info));

        let range_req = semver::VersionReq::parse(">=4.0").unwrap();
        assert!(!PythonRequest::Range(range_req, ">=4.0".to_string()).satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn prerelease_requests_match_exactly() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Python, None, Vec::new(), temp_dir.path())?;
        install_info
            .with_language_version(semver::Version::parse("3.13.0-rc.1")?)
            .with_toolchain(PathBuf::from("/usr/bin/python3.13"));

        let rc1 = PythonRequest::from_str("3.13.0rc1")?;
        assert!(rc1.satisfied_by(&install_info));

        // A different prerelease, or the final release, must not reuse an rc1 env.
        assert!(!PythonRequest::from_str("3.13.0rc2")?.satisfied_by(&install_info));
        assert!(!PythonRequest::MajorMinorPatch(3, 13, 0).satisfied_by(&install_info));

        Ok(())
    }
}
