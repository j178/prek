use std::fmt::Display;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct RustVersion(semver::Version);

impl Default for RustVersion {
    fn default() -> Self {
        RustVersion(semver::Version::new(0, 0, 0))
    }
}

impl Deref for RustVersion {
    type Target = semver::Version;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for RustVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl FromStr for RustVersion {
    type Err = semver::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let s = s.trim();
        semver::Version::parse(s).map(RustVersion)
    }
}

/// `language_version` field of rust can be one of the following:
/// `default`
/// `system`
/// `stable`
/// `nightly`
/// `beta`
/// `1.70` or `1.70.0`
/// `>= 1.70, < 1.72`
/// `local/path/to/rust`
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum RustRequest {
    Any,
    Channel(String),
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Path(PathBuf),
    Range(semver::VersionReq, String),
}

impl FromStr for RustRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(RustRequest::Any);
        }

        // Check for channel names
        if s == "stable" || s == "nightly" || s == "beta" {
            return Ok(RustRequest::Channel(s.to_string()));
        }

        // Try parsing as version numbers
        Self::parse_version_numbers(s, s)
            .or_else(|_| {
                semver::VersionReq::parse(s)
                    .map(|version_req| RustRequest::Range(version_req, s.into()))
                    .map_err(|_| Error::InvalidVersion(s.to_string()))
            })
            .or_else(|_| {
                let path = PathBuf::from(s);
                if path.exists() {
                    Ok(RustRequest::Path(path))
                } else {
                    Err(Error::InvalidVersion(s.to_string()))
                }
            })
    }
}

impl RustRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, RustRequest::Any)
    }

    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> Result<RustRequest, Error> {
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(original_request.to_string()))?;

        match parts.as_slice() {
            [major] => Ok(RustRequest::Major(*major)),
            [major, minor] => Ok(RustRequest::MajorMinor(*major, *minor)),
            [major, minor, patch] => Ok(RustRequest::MajorMinorPatch(*major, *minor, *patch)),
            _ => Err(Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            RustRequest::Any => {
                // Any request accepts any valid installation, or specifically "stable"
                install_info
                    .get_extra("rust_channel")
                    .is_some_and(|ch| ch == "stable")
                    || install_info.language_version.major > 0
            }
            RustRequest::Channel(requested_channel) => install_info
                .get_extra("rust_channel")
                .is_some_and(|ch| ch == requested_channel),
            _ => {
                let version = &install_info.language_version;
                self.matches(
                    &RustVersion(version.clone()),
                    Some(install_info.toolchain.as_ref()),
                )
            }
        }
    }

    pub(crate) fn matches(&self, version: &RustVersion, toolchain: Option<&Path>) -> bool {
        match self {
            RustRequest::Any => true,
            RustRequest::Channel(_requested_channel) => {
                // Cannot match channel names against specific version numbers
                // e.g. request "stable" against version "1.70.0"
                // TODO: Resolve channel by querying rustup or Rust release API
                false
            }
            RustRequest::Major(major) => version.0.major == *major,
            RustRequest::MajorMinor(major, minor) => {
                version.0.major == *major && version.0.minor == *minor
            }
            RustRequest::MajorMinorPatch(major, minor, patch) => {
                version.0.major == *major && version.0.minor == *minor && version.0.patch == *patch
            }
            RustRequest::Path(path) => toolchain.is_some_and(|t| t == path),
            RustRequest::Range(req, _) => req.matches(&version.0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Language;
    use crate::hook::InstallInfo;
    use rustc_hash::FxHashSet;
    use std::str::FromStr;

    #[test]
    fn test_request_from_str() -> anyhow::Result<()> {
        assert_eq!(RustRequest::from_str("")?, RustRequest::Any);
        assert_eq!(
            RustRequest::from_str("stable")?,
            RustRequest::Channel("stable".into())
        );
        assert_eq!(
            RustRequest::from_str("beta")?,
            RustRequest::Channel("beta".into())
        );
        assert_eq!(
            RustRequest::from_str("nightly")?,
            RustRequest::Channel("nightly".into())
        );
        assert_eq!(RustRequest::from_str("1")?, RustRequest::Major(1));
        assert_eq!(
            RustRequest::from_str("1.70")?,
            RustRequest::MajorMinor(1, 70)
        );
        assert_eq!(
            RustRequest::from_str("1.70.1")?,
            RustRequest::MajorMinorPatch(1, 70, 1)
        );

        let range_str = ">=1.70, <1.72";
        assert_eq!(
            RustRequest::from_str(range_str)?,
            RustRequest::Range(semver::VersionReq::parse(range_str)?, range_str.into())
        );

        let temp_dir = tempfile::tempdir()?;
        let toolchain_path = temp_dir.path().join("rust-toolchain");
        std::fs::write(&toolchain_path, b"")?;
        let path_request = RustRequest::from_str(toolchain_path.to_str().unwrap())?;
        assert_eq!(path_request, RustRequest::Path(toolchain_path.clone()));

        Ok(())
    }

    #[test]
    fn test_invalid_requests() {
        assert!(RustRequest::from_str("unknown-channel").is_err());
        assert!(RustRequest::from_str("1.2.3.4").is_err());
        assert!(RustRequest::from_str("1.2.a").is_err());
        assert!(RustRequest::from_str("/non/existent/path/to/rust").is_err());
    }

    #[test]
    fn test_request_matches() -> anyhow::Result<()> {
        let version = RustVersion::from_str("1.71.0")?;
        let other_version = RustVersion::from_str("1.72.1")?;

        assert!(RustRequest::Any.matches(&version, None));
        assert!(!RustRequest::Channel("stable".into()).matches(&version, None));
        assert!(RustRequest::Major(1).matches(&version, None));
        assert!(!RustRequest::Major(2).matches(&version, None));
        assert!(RustRequest::MajorMinor(1, 71).matches(&version, None));
        assert!(!RustRequest::MajorMinor(1, 72).matches(&version, None));
        assert!(RustRequest::MajorMinorPatch(1, 71, 0).matches(&version, None));
        assert!(!RustRequest::MajorMinorPatch(1, 71, 1).matches(&version, None));

        let temp_dir = tempfile::tempdir()?;
        let toolchain_path = temp_dir.path().join("rust-toolchain");
        std::fs::write(&toolchain_path, b"")?;

        assert!(RustRequest::Path(toolchain_path.clone()).matches(&version, Some(&toolchain_path)));
        assert!(!RustRequest::Path(toolchain_path.clone()).matches(&version, None));

        let req = semver::VersionReq::parse(">=1.70, <1.72")?;
        assert!(RustRequest::Range(req.clone(), ">=1.70, <1.72".into()).matches(&version, None));
        assert!(!RustRequest::Range(req, ">=1.70, <1.72".into()).matches(&other_version, None));

        Ok(())
    }

    #[test]
    fn test_request_satisfied_by_install_info() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let toolchain_path = temp_dir.path().join("rust-toolchain");
        std::fs::write(&toolchain_path, b"")?;

        let mut install_info =
            InstallInfo::new(Language::Rust, FxHashSet::default(), temp_dir.path())?;
        install_info
            .with_language_version(semver::Version::new(1, 71, 0))
            .with_toolchain(toolchain_path.clone());

        assert!(RustRequest::Any.satisfied_by(&install_info));
        assert!(RustRequest::Major(1).satisfied_by(&install_info));
        assert!(RustRequest::MajorMinor(1, 71).satisfied_by(&install_info));
        assert!(RustRequest::MajorMinorPatch(1, 71, 0).satisfied_by(&install_info));
        assert!(!RustRequest::MajorMinorPatch(1, 71, 1).satisfied_by(&install_info));
        assert!(RustRequest::Path(toolchain_path.clone()).satisfied_by(&install_info));

        let req = RustRequest::Range(
            semver::VersionReq::parse(">=1.70, <1.72")?,
            ">=1.70, <1.72".into(),
        );
        assert!(req.satisfied_by(&install_info));

        let req = RustRequest::Range(semver::VersionReq::parse(">=1.72")?, ">=1.72".into());
        assert!(!req.satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn test_satisfied_by_channel() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::new(Language::Rust, FxHashSet::default(), temp_dir.path())?;
        install_info
            .with_language_version(semver::Version::new(1, 75, 0))
            .with_toolchain(PathBuf::from("/some/path"))
            .with_extra("rust_channel", "stable");

        // Channel request should match when extra is set
        assert!(RustRequest::Channel("stable".into()).satisfied_by(&install_info));
        assert!(!RustRequest::Channel("nightly".into()).satisfied_by(&install_info));
        assert!(!RustRequest::Channel("beta".into()).satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn test_satisfied_by_any_with_stable_channel() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::new(Language::Rust, FxHashSet::default(), temp_dir.path())?;
        install_info
            .with_language_version(semver::Version::new(1, 75, 0))
            .with_toolchain(PathBuf::from("/some/path"))
            .with_extra("rust_channel", "stable");

        // Any request should match stable channel
        assert!(RustRequest::Any.satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn test_satisfied_by_any_without_channel() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::new(Language::Rust, FxHashSet::default(), temp_dir.path())?;
        install_info
            .with_language_version(semver::Version::new(1, 75, 0))
            .with_toolchain(PathBuf::from("/some/path"));
        // No channel set - should still match Any if version > 0

        assert!(RustRequest::Any.satisfied_by(&install_info));

        Ok(())
    }
}
