//! .NET SDK version request parsing.
//!
//! Supports version formats like:
//! - `8.0` or `8.0.100` - specific version
//! - `8` - major version only
//! - `net8.0`, `net9.0`, or `net10.0` - TFM-style versions
use std::str::FromStr;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DotnetRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
}

impl FromStr for DotnetRequest {
    type Err = Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        if request.is_empty() {
            return Ok(Self::Any);
        }

        // Handle TFM-style versions like "net8.0", "net9.0", or "net10.0"
        let version_str = request
            .strip_prefix("net")
            .or_else(|| request.strip_prefix("dotnet"))
            .unwrap_or(request);

        if version_str.is_empty() {
            return Ok(Self::Any);
        }

        Self::parse_version_numbers(version_str, request)
    }
}

impl DotnetRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, DotnetRequest::Any)
    }

    fn parse_version_numbers(version_str: &str, original_request: &str) -> Result<Self, Error> {
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(original_request.to_string()))?;

        match parts[..] {
            [major] => Ok(DotnetRequest::Major(major)),
            [major, minor] => Ok(DotnetRequest::MajorMinor(major, minor)),
            [major, minor, patch] => Ok(DotnetRequest::MajorMinorPatch(major, minor, patch)),
            _ => Err(Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = &install_info.language_version;
        match self {
            DotnetRequest::Any => true,
            DotnetRequest::Major(major) => version.major == *major,
            DotnetRequest::MajorMinor(major, minor) => {
                version.major == *major && version.minor == *minor
            }
            DotnetRequest::MajorMinorPatch(major, minor, patch) => {
                version.major == *major && version.minor == *minor && version.patch == *patch
            }
        }
    }

    /// Convert to a version string suitable for dotnet-install script.
    pub(crate) fn to_install_version(&self) -> Option<String> {
        match self {
            DotnetRequest::Any => None,
            DotnetRequest::Major(major) => Some(format!("{major}.0")),
            DotnetRequest::MajorMinor(major, minor) => Some(format!("{major}.{minor}")),
            DotnetRequest::MajorMinorPatch(major, minor, patch) => {
                Some(format!("{major}.{minor}.{patch}"))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Language;
    use rustc_hash::FxHashSet;
    use std::path::PathBuf;

    #[test]
    fn test_parse_dotnet_request() {
        // Empty request
        assert_eq!(DotnetRequest::from_str("").unwrap(), DotnetRequest::Any);

        // Major only
        assert_eq!(
            DotnetRequest::from_str("8").unwrap(),
            DotnetRequest::Major(8)
        );

        // Major.minor
        assert_eq!(
            DotnetRequest::from_str("8.0").unwrap(),
            DotnetRequest::MajorMinor(8, 0)
        );
        assert_eq!(
            DotnetRequest::from_str("9.0").unwrap(),
            DotnetRequest::MajorMinor(9, 0)
        );

        // Full version
        assert_eq!(
            DotnetRequest::from_str("8.0.100").unwrap(),
            DotnetRequest::MajorMinorPatch(8, 0, 100)
        );

        // TFM-style versions
        assert_eq!(
            DotnetRequest::from_str("net8.0").unwrap(),
            DotnetRequest::MajorMinor(8, 0)
        );
        assert_eq!(
            DotnetRequest::from_str("net9.0").unwrap(),
            DotnetRequest::MajorMinor(9, 0)
        );
        assert_eq!(
            DotnetRequest::from_str("net10.0").unwrap(),
            DotnetRequest::MajorMinor(10, 0)
        );

        // dotnet prefix
        assert_eq!(
            DotnetRequest::from_str("dotnet8.0").unwrap(),
            DotnetRequest::MajorMinor(8, 0)
        );

        // Invalid versions
        assert!(DotnetRequest::from_str("invalid").is_err());
        assert!(DotnetRequest::from_str("8.0.100.1").is_err());
        assert!(DotnetRequest::from_str("8.a").is_err());
    }

    #[test]
    fn test_is_any() {
        assert!(DotnetRequest::Any.is_any());
        assert!(!DotnetRequest::Major(8).is_any());
        assert!(!DotnetRequest::MajorMinor(8, 0).is_any());
        assert!(!DotnetRequest::MajorMinorPatch(8, 0, 100).is_any());
    }

    #[test]
    fn test_parse_net_prefix_only() {
        // "net" alone should return Any
        assert_eq!(DotnetRequest::from_str("net").unwrap(), DotnetRequest::Any);
        assert_eq!(
            DotnetRequest::from_str("dotnet").unwrap(),
            DotnetRequest::Any
        );
    }

    #[test]
    fn test_satisfied_by() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::new(Language::Dotnet, FxHashSet::default(), temp_dir.path())?;
        install_info
            .with_language_version(semver::Version::new(8, 0, 100))
            .with_toolchain(PathBuf::from("/usr/share/dotnet/dotnet"));

        assert!(DotnetRequest::Any.satisfied_by(&install_info));
        assert!(DotnetRequest::Major(8).satisfied_by(&install_info));
        assert!(DotnetRequest::MajorMinor(8, 0).satisfied_by(&install_info));
        assert!(DotnetRequest::MajorMinorPatch(8, 0, 100).satisfied_by(&install_info));
        assert!(!DotnetRequest::MajorMinorPatch(8, 0, 101).satisfied_by(&install_info));
        assert!(!DotnetRequest::Major(9).satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn test_to_install_version() {
        assert_eq!(DotnetRequest::Any.to_install_version(), None);
        assert_eq!(
            DotnetRequest::Major(8).to_install_version(),
            Some("8.0".to_string())
        );
        assert_eq!(
            DotnetRequest::MajorMinor(8, 0).to_install_version(),
            Some("8.0".to_string())
        );
        assert_eq!(
            DotnetRequest::MajorMinorPatch(8, 0, 100).to_install_version(),
            Some("8.0.100".to_string())
        );
    }
}
