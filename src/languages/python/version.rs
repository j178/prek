//! Implement `-p <python_spec>` argument parser of `virutualenv` from
//! <https://github.com/pypa/virtualenv/blob/216dc9f3592aa1f3345290702f0e7ba3432af3ce/src/virtualenv/discovery/py_spec.py>
use std::path::PathBuf;
use std::str::FromStr;

use crate::hook::InstallInfo;
use crate::languages::version;
use crate::languages::version::try_into_u64_slice;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PythonRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Path(PathBuf),
    Range(semver::VersionReq, String),
}

/// Represents a request for a specific Python version or path.
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
/// - `/path/to/python`
/// - `/path/to/python3.12`
// TODO: support version like `3.8b1`, `3.8rc2`, `python3.8t`, `python3.8-64`, `pypy3.8`.
impl FromStr for PythonRequest {
    type Err = version::Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        if request.is_empty() {
            return Ok(Self::Any);
        }

        // Check if it starts with "python" - parse as specific version
        if let Some(version_part) = request.strip_prefix("python") {
            if version_part.is_empty() {
                return Ok(Self::Any);
            }

            Self::parse_version_numbers(version_part, request)
        } else {
            Self::parse_version_numbers(request, request)
                .or_else(|_| {
                    // Try to parse as a VersionReq (like ">= 3.12" or ">=3.8, <3.12")
                    semver::VersionReq::parse(request)
                        .map(|version_req| PythonRequest::Range(version_req, request.into()))
                        .map_err(|_| version::Error::InvalidVersion(request.to_string()))
                })
                .or_else(|_| {
                    // If it doesn't match any known format, treat it as a path
                    let path = PathBuf::from(request);
                    if path.exists() {
                        Ok(PythonRequest::Path(path))
                    } else {
                        Err(version::Error::InvalidVersion(request.to_string()))
                    }
                })
        }
    }
}

impl PythonRequest {
    /// Parse version numbers into appropriate `PythonRequest` variants
    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> Result<PythonRequest, version::Error> {
        // Check if all parts are valid u8 numbers
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| version::Error::InvalidVersion(original_request.to_string()))?;
        let parts = split_wheel_tag_version(parts);

        match parts[..] {
            [major] => Ok(PythonRequest::Major(major)),
            [major, minor] => Ok(PythonRequest::MajorMinor(major, minor)),
            [major, minor, patch] => Ok(PythonRequest::MajorMinorPatch(major, minor, patch)),
            _ => Err(version::Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = &install_info.language_version;
        match self {
            PythonRequest::Any => true,
            PythonRequest::Major(major) => version.major == *major,
            PythonRequest::MajorMinor(major, minor) => {
                version.major == *major && version.minor == *minor
            }
            PythonRequest::MajorMinorPatch(major, minor, patch) => {
                version.major == *major && version.minor == *minor && version.patch == *patch
            }
            PythonRequest::Path(path) => path == &install_info.toolchain,
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Language;
    use std::collections::HashSet;
    use std::path::Path;

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

        // TODO: support
        assert!(PythonRequest::from_str("3.12.3a1").is_err(),);
        assert!(PythonRequest::from_str("3.12.3rc1").is_err(),);
        assert!(PythonRequest::from_str("python3.13.2a1").is_err());
        assert!(PythonRequest::from_str("python3.13.2rc1").is_err());
        assert!(PythonRequest::from_str("python3.13.2t1").is_err());
        assert!(PythonRequest::from_str("python3.13.2-64").is_err());
        assert!(PythonRequest::from_str("python3.13.2-64").is_err());
    }

    #[test]
    fn test_satisfied_by() {
        let mut install_info =
            InstallInfo::new(Language::Python, HashSet::default(), Path::new("."));
        install_info
            .with_language_version(semver::Version::new(3, 12, 1))
            .with_toolchain(PathBuf::from("/usr/bin/python3.12"));

        assert!(PythonRequest::Any.satisfied_by(&install_info));
        assert!(PythonRequest::Major(3).satisfied_by(&install_info));
        assert!(PythonRequest::MajorMinor(3, 12).satisfied_by(&install_info));
        assert!(PythonRequest::MajorMinorPatch(3, 12, 1).satisfied_by(&install_info));
        assert!(!PythonRequest::MajorMinorPatch(3, 12, 2).satisfied_by(&install_info));
        assert!(
            PythonRequest::Path(PathBuf::from("/usr/bin/python3.12")).satisfied_by(&install_info)
        );
        assert!(
            !PythonRequest::Path(PathBuf::from("/usr/bin/python3.11")).satisfied_by(&install_info)
        );

        let range_req = semver::VersionReq::parse(">=3.12").unwrap();
        assert!(
            PythonRequest::Range(range_req.clone(), ">=3.12".to_string())
                .satisfied_by(&install_info)
        );

        let range_req = semver::VersionReq::parse(">=4.0").unwrap();
        assert!(!PythonRequest::Range(range_req, ">=4.0".to_string()).satisfied_by(&install_info));
    }
}
