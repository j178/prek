use std::fmt::Display;
use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum Channel {
    Stable,
    Beta,
    Nightly,
}

impl FromStr for Channel {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "stable" => Ok(Channel::Stable),
            "beta" => Ok(Channel::Beta),
            "nightly" => Ok(Channel::Nightly),
            _ => Err(()),
        }
    }
}

impl Display for Channel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let channel_str = match self {
            Channel::Stable => "stable",
            Channel::Beta => "beta",
            Channel::Nightly => "nightly",
        };
        write!(f, "{channel_str}")
    }
}

#[derive(Debug, Clone)]
pub(crate) struct RustVersion {
    version: semver::Version,
    channel: Option<Channel>,
}

impl Default for RustVersion {
    fn default() -> Self {
        Self {
            version: semver::Version::new(0, 0, 0),
            channel: None,
        }
    }
}

impl Deref for RustVersion {
    type Target = semver::Version;

    fn deref(&self) -> &Self::Target {
        &self.version
    }
}

impl RustVersion {
    pub(crate) fn from_version(version: &semver::Version) -> Self {
        Self {
            version: version.clone(),
            channel: None,
        }
    }

    pub(crate) fn from_channel(channel: Channel) -> Self {
        Self {
            version: semver::Version::new(0, 0, 0),
            channel: Some(channel),
        }
    }

    pub(crate) fn from_path(version: &semver::Version, path: &Path) -> Self {
        let toolchain_str = path
            .file_name()
            .and_then(|os_str| os_str.to_str())
            .unwrap_or_default();
        let path = toolchain_str.to_lowercase();
        let channel = if path.starts_with("nightly") {
            Some(Channel::Nightly)
        } else if path.starts_with("beta") {
            Some(Channel::Beta)
        } else if path.starts_with("stable") {
            Some(Channel::Stable)
        } else {
            None
        };
        Self {
            version: version.clone(),
            channel,
        }
    }

    pub(crate) fn to_toolchain_name(&self) -> String {
        if let Some(channel) = &self.channel {
            channel.to_string()
        } else {
            format!(
                "{}.{}.{}",
                self.version.major, self.version.minor, self.version.patch
            )
        }
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
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum RustRequest {
    Any,
    Channel(Channel),
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Range(semver::VersionReq, String),
}

impl FromStr for RustRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        if s.is_empty() {
            return Ok(RustRequest::Any);
        }

        // Check for channel names
        if let Ok(channel) = Channel::from_str(s) {
            return Ok(RustRequest::Channel(channel));
        }

        // Try parsing as version numbers
        Self::parse_version_numbers(s, s).or_else(|_| {
            semver::VersionReq::parse(s)
                .map(|version_req| RustRequest::Range(version_req, s.into()))
                .map_err(|_| Error::InvalidVersion(s.to_string()))
        })
    }
}

impl Display for RustRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RustRequest::Any => write!(f, "any"),
            RustRequest::Channel(channel) => write!(f, "{channel}"),
            RustRequest::Major(major) => write!(f, "{major}"),
            RustRequest::MajorMinor(major, minor) => write!(f, "{major}.{minor}"),
            RustRequest::MajorMinorPatch(major, minor, patch) => {
                write!(f, "{major}.{minor}.{patch}")
            }
            RustRequest::Range(_, range_str) => write!(f, "{range_str}"),
        }
    }
}

pub(crate) const EXTRA_KEY_CHANNEL: &str = "channel";

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
                    .get_extra(EXTRA_KEY_CHANNEL)
                    .is_some_and(|ch| ch == "stable")
                    || install_info.language_version.major > 0
            }
            RustRequest::Channel(requested_channel) => {
                let channel = install_info
                    .get_extra(EXTRA_KEY_CHANNEL)
                    .and_then(|ch| Channel::from_str(ch).ok());
                channel.as_ref().is_some_and(|ch| ch == requested_channel)
            }
            _ => {
                let version = &install_info.language_version;
                self.matches(
                    &RustVersion::from_version(version),
                    Some(install_info.toolchain.as_ref()),
                )
            }
        }
    }

    pub(crate) fn matches(&self, version: &RustVersion, _toolchain: Option<&Path>) -> bool {
        match self {
            RustRequest::Any => true,
            RustRequest::Channel(requested_channel) => version
                .channel
                .as_ref()
                .is_some_and(|ch| ch == requested_channel),
            RustRequest::Major(major) => version.version.major == *major,
            RustRequest::MajorMinor(major, minor) => {
                version.version.major == *major && version.version.minor == *minor
            }
            RustRequest::MajorMinorPatch(major, minor, patch) => {
                version.version.major == *major
                    && version.version.minor == *minor
                    && version.version.patch == *patch
            }
            RustRequest::Range(req, _) => req.matches(&version.version),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Language;
    use crate::hook::InstallInfo;
    use rustc_hash::FxHashSet;
    use std::path::PathBuf;
    use std::str::FromStr;

    #[test]
    fn test_request_from_str() -> anyhow::Result<()> {
        assert_eq!(RustRequest::from_str("")?, RustRequest::Any);
        assert_eq!(
            RustRequest::from_str("stable")?,
            RustRequest::Channel(Channel::Stable)
        );
        assert_eq!(
            RustRequest::from_str("beta")?,
            RustRequest::Channel(Channel::Beta)
        );
        assert_eq!(
            RustRequest::from_str("nightly")?,
            RustRequest::Channel(Channel::Nightly)
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
        let version = RustVersion::from_path(
            &semver::Version::new(1, 71, 0),
            Path::new("/home/user/.rustup/toolchains/stable-x86_64-unknown-linux-gnu"),
        );
        let other_version = RustVersion::from_version(&semver::Version::new(1, 72, 1));

        assert!(RustRequest::Any.matches(&version, None));
        assert!(RustRequest::Channel(Channel::Stable).matches(&version, None));
        assert!(!RustRequest::Channel(Channel::Stable).matches(&other_version, None));
        assert!(RustRequest::Major(1).matches(&version, None));
        assert!(!RustRequest::Major(2).matches(&version, None));
        assert!(RustRequest::MajorMinor(1, 71).matches(&version, None));
        assert!(!RustRequest::MajorMinor(1, 72).matches(&version, None));
        assert!(RustRequest::MajorMinorPatch(1, 71, 0).matches(&version, None));
        assert!(!RustRequest::MajorMinorPatch(1, 71, 1).matches(&version, None));

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
            .with_extra(EXTRA_KEY_CHANNEL, "stable");

        // Channel request should match when extra is set
        assert!(RustRequest::Channel(Channel::Stable).satisfied_by(&install_info));
        assert!(!RustRequest::Channel(Channel::Nightly).satisfied_by(&install_info));
        assert!(!RustRequest::Channel(Channel::Beta).satisfied_by(&install_info));

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
