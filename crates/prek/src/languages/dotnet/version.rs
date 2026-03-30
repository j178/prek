//! .NET SDK version request parsing.
//!
//! Supports version formats like:
//! - `8.0` or `8.0.100` - channel or exact version
//! - `8` - major version only
//! - `8.0.1xx` - SDK feature band channel
//! - `net8.0` - prefixed channel requests
//! - `lts`, `sts` - release track requests
use std::fmt::Display;
use std::ops::Deref;
use std::str::FromStr;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

/// A parsed `.NET SDK` version.
///
/// This wraps [`semver::Version`] but accepts the two-part SDK strings that
/// `dotnet --version` and user requests commonly use, such as `8.0`.
#[derive(Debug, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub(crate) struct DotnetVersion(semver::Version);

impl Default for DotnetVersion {
    fn default() -> Self {
        Self(semver::Version::new(0, 0, 0))
    }
}

impl Deref for DotnetVersion {
    type Target = semver::Version;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for DotnetVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<DotnetVersion> for semver::Version {
    fn from(version: DotnetVersion) -> Self {
        version.0
    }
}

impl FromStr for DotnetVersion {
    type Err = Error;

    fn from_str(version: &str) -> Result<Self, Self::Err> {
        let version = version.trim();
        let base_version = version.split('-').next().unwrap_or(version);
        let parts = try_into_u64_slice(base_version)
            .map_err(|_| Error::InvalidVersion(version.to_string()))?;

        match parts.as_slice() {
            [major, minor] => Ok(Self(semver::Version::new(*major, *minor, 0))),
            [major, minor, patch] => Ok(Self(semver::Version::new(*major, *minor, *patch))),
            _ => Err(Error::InvalidVersion(version.to_string())),
        }
    }
}

impl DotnetVersion {
    /// Creates a new parsed SDK version.
    pub(crate) fn new(major: u64, minor: u64, patch: u64) -> Self {
        Self(semver::Version::new(major, minor, patch))
    }

    /// Returns the SDK feature band encoded in the patch component.
    ///
    /// For example, `8.0.203` belongs to feature band `2xx`.
    fn feature_band(&self) -> u64 {
        self.patch / 100
    }

    /// Returns the support track for this SDK line when it can be inferred locally.
    ///
    /// This is used for request matching only. It intentionally does not try to
    /// resolve the "latest STS/LTS" channel from remote release metadata.
    fn release_track(&self) -> Option<DotnetReleaseTrack> {
        match (self.major, self.minor) {
            (1, 0 | 1) | (2 | 3, 1) => Some(DotnetReleaseTrack::Lts),
            (2, 0 | 2) | (3, 0) => Some(DotnetReleaseTrack::Sts),
            (major, 0) if major >= 5 => Some(if major % 2 == 0 {
                DotnetReleaseTrack::Lts
            } else {
                DotnetReleaseTrack::Sts
            }),
            _ => None,
        }
    }
}

/// The support track inferred from a concrete installed SDK version.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DotnetReleaseTrack {
    /// Long Term Support.
    Lts,
    /// Standard Term Support.
    Sts,
}

/// A `dotnet-install` channel request.
///
/// This mirrors the channel forms accepted by the install script, including
/// numeric channels (`8.0`), feature bands (`8.0.1xx`), and support tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DotnetChannel {
    /// A major.minor SDK channel such as `8.0`.
    Version(u64, u64),
    /// A feature-band channel such as `8.0.1xx`.
    FeatureBand(u64, u64, u64),
    /// The latest Long Term Support channel.
    Lts,
    /// The latest Standard Term Support channel.
    Sts,
}

impl Display for DotnetChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Version(major, minor) => write!(f, "{major}.{minor}"),
            Self::FeatureBand(major, minor, band) => write!(f, "{major}.{minor}.{band}xx"),
            Self::Lts => write!(f, "LTS"),
            Self::Sts => write!(f, "STS"),
        }
    }
}

impl DotnetChannel {
    /// Returns whether a concrete SDK version satisfies this channel request.
    fn matches(&self, version: &DotnetVersion) -> bool {
        match self {
            Self::Version(major, minor) => version.major == *major && version.minor == *minor,
            Self::FeatureBand(major, minor, band) => {
                version.major == *major
                    && version.minor == *minor
                    && version.feature_band() == *band
            }
            Self::Lts => version.release_track() == Some(DotnetReleaseTrack::Lts),
            Self::Sts => version.release_track() == Some(DotnetReleaseTrack::Sts),
        }
    }
}

/// A parsed `language_version` request for the `dotnet` language backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum DotnetRequest {
    /// Accept any available SDK, preferring the default LTS install channel.
    Any,
    /// Resolve or match a channel-style request.
    Channel(DotnetChannel),
    /// Require one exact SDK version such as `8.0.100`.
    Exact(u64, u64, u64),
}

impl FromStr for DotnetRequest {
    type Err = Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        if request.is_empty() {
            return Ok(Self::Any);
        }

        let version_str = request.strip_prefix("net").unwrap_or(request);

        if version_str.is_empty() {
            return Ok(Self::Any);
        }

        if version_str.eq_ignore_ascii_case("lts") {
            return Ok(Self::Channel(DotnetChannel::Lts));
        }
        if version_str.eq_ignore_ascii_case("sts") {
            return Ok(Self::Channel(DotnetChannel::Sts));
        }

        if let Some(channel) = Self::parse_feature_band(version_str) {
            return Ok(Self::Channel(channel));
        }

        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(request.to_string()))?;

        match parts.as_slice() {
            [major] => Ok(DotnetRequest::Channel(DotnetChannel::Version(*major, 0))),
            [major, minor] => Ok(DotnetRequest::Channel(DotnetChannel::Version(
                *major, *minor,
            ))),
            [major, minor, patch] => Ok(DotnetRequest::Exact(*major, *minor, *patch)),
            _ => Err(Error::InvalidVersion(request.to_string())),
        }
    }
}

impl DotnetRequest {
    /// Returns whether this request accepts any SDK.
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, DotnetRequest::Any)
    }

    /// Parses a feature-band channel like `8.0.1xx`.
    fn parse_feature_band(version_str: &str) -> Option<DotnetChannel> {
        let (prefix, feature_band) = version_str.split_once('.')?;
        let (minor, band_suffix) = feature_band.split_once('.')?;
        let band = band_suffix.strip_suffix("xx")?;
        let major = prefix.parse().ok()?;
        let minor = minor.parse().ok()?;
        let band = band.parse().ok()?;

        Some(DotnetChannel::FeatureBand(major, minor, band))
    }

    /// Returns whether a persisted installation satisfies this request.
    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = DotnetVersion(install_info.language_version.clone());
        self.matches(&version)
    }

    /// Returns whether a concrete SDK version satisfies this request.
    pub(crate) fn matches(&self, version: &DotnetVersion) -> bool {
        match self {
            DotnetRequest::Any => true,
            DotnetRequest::Channel(channel) => channel.matches(version),
            DotnetRequest::Exact(major, minor, patch) => {
                version.major == *major && version.minor == *minor && version.patch == *patch
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rustc_hash::FxHashSet;

    use super::*;
    use crate::config::Language;
    use crate::languages::version::LanguageRequest;

    #[test]
    fn test_parse_dotnet_request() {
        // Empty request
        assert_eq!(DotnetRequest::from_str("").unwrap(), DotnetRequest::Any);

        // Major only
        assert_eq!(
            DotnetRequest::from_str("8").unwrap(),
            DotnetRequest::Channel(DotnetChannel::Version(8, 0))
        );

        // Major.minor
        assert_eq!(
            DotnetRequest::from_str("8.0").unwrap(),
            DotnetRequest::Channel(DotnetChannel::Version(8, 0))
        );
        assert_eq!(
            DotnetRequest::from_str("9.0").unwrap(),
            DotnetRequest::Channel(DotnetChannel::Version(9, 0))
        );

        // Full version
        assert_eq!(
            DotnetRequest::from_str("8.0.100").unwrap(),
            DotnetRequest::Exact(8, 0, 100)
        );
        assert_eq!(
            DotnetRequest::from_str("8.0.1xx").unwrap(),
            DotnetRequest::Channel(DotnetChannel::FeatureBand(8, 0, 1))
        );
        assert_eq!(
            DotnetRequest::from_str("8.0.4xx").unwrap(),
            DotnetRequest::Channel(DotnetChannel::FeatureBand(8, 0, 4))
        );

        // TFM-style versions
        assert_eq!(
            DotnetRequest::from_str("net8.0").unwrap(),
            DotnetRequest::Channel(DotnetChannel::Version(8, 0))
        );
        assert_eq!(
            DotnetRequest::from_str("net8.0.1xx").unwrap(),
            DotnetRequest::Channel(DotnetChannel::FeatureBand(8, 0, 1))
        );
        assert_eq!(
            DotnetRequest::from_str("net9.0").unwrap(),
            DotnetRequest::Channel(DotnetChannel::Version(9, 0))
        );
        assert_eq!(
            DotnetRequest::from_str("net10.0").unwrap(),
            DotnetRequest::Channel(DotnetChannel::Version(10, 0))
        );

        // release tracks
        assert_eq!(
            DotnetRequest::from_str("lts").unwrap(),
            DotnetRequest::Channel(DotnetChannel::Lts)
        );
        assert_eq!(
            DotnetRequest::from_str("STS").unwrap(),
            DotnetRequest::Channel(DotnetChannel::Sts)
        );

        // Invalid versions
        assert!(DotnetRequest::from_str("invalid").is_err());
        assert!(DotnetRequest::from_str("8.0.100.1").is_err());
        assert!(DotnetRequest::from_str("8.a").is_err());
        assert!(DotnetRequest::from_str("8.0.xx").is_err());
        assert!(DotnetRequest::from_str("8.0.1x").is_err());
        assert!(DotnetRequest::from_str("dotnet").is_err());
        assert!(DotnetRequest::from_str("dotnet8.0").is_err());
    }

    #[test]
    fn test_parse_dotnet_version() {
        assert_eq!(
            "8.0.100".parse::<DotnetVersion>().unwrap(),
            DotnetVersion::new(8, 0, 100)
        );
        assert_eq!(
            "10.0.1".parse::<DotnetVersion>().unwrap(),
            DotnetVersion::new(10, 0, 1)
        );
        assert_eq!(
            "8.0".parse::<DotnetVersion>().unwrap(),
            DotnetVersion::new(8, 0, 0)
        );
        assert_eq!(
            "8.0.100-preview.1".parse::<DotnetVersion>().unwrap(),
            DotnetVersion::new(8, 0, 100)
        );
        assert!("invalid".parse::<DotnetVersion>().is_err());
        assert!("8".parse::<DotnetVersion>().is_err());
        assert!("".parse::<DotnetVersion>().is_err());
    }

    #[test]
    fn test_is_any() {
        assert!(DotnetRequest::Any.is_any());
        assert!(!DotnetRequest::Channel(DotnetChannel::Version(8, 0)).is_any());
        assert!(!DotnetRequest::Exact(8, 0, 100).is_any());

        // Test through LanguageRequest dispatch
        let req = LanguageRequest::parse(Language::Dotnet, "net").unwrap();
        assert!(req.is_any());
        let req = LanguageRequest::parse(Language::Dotnet, "8").unwrap();
        assert!(!req.is_any());
    }

    #[test]
    fn test_parse_net_prefix_only() {
        // "net" alone should return Any
        assert_eq!(DotnetRequest::from_str("net").unwrap(), DotnetRequest::Any);
    }

    #[test]
    fn test_matches() {
        let version = DotnetVersion::new(8, 0, 100);
        let feature_band_version = DotnetVersion::new(8, 0, 199);
        let next_feature_band_version = DotnetVersion::new(8, 0, 203);
        let sts_version = DotnetVersion::new(9, 0, 100);
        let legacy_lts = DotnetVersion::new(3, 1, 426);
        let legacy_sts = DotnetVersion::new(3, 0, 103);

        assert!(DotnetRequest::Any.matches(&version));
        assert!(DotnetRequest::Channel(DotnetChannel::Version(8, 0)).matches(&version));
        assert!(!DotnetRequest::Channel(DotnetChannel::Version(9, 0)).matches(&version));
        assert!(!DotnetRequest::Channel(DotnetChannel::Version(8, 1)).matches(&version));
        assert!(
            DotnetRequest::Channel(DotnetChannel::FeatureBand(8, 0, 1))
                .matches(&feature_band_version)
        );
        assert!(
            !DotnetRequest::Channel(DotnetChannel::FeatureBand(8, 0, 2))
                .matches(&feature_band_version)
        );
        assert!(
            DotnetRequest::Channel(DotnetChannel::FeatureBand(8, 0, 2))
                .matches(&next_feature_band_version)
        );
        assert!(DotnetRequest::Channel(DotnetChannel::Lts).matches(&version));
        assert!(!DotnetRequest::Channel(DotnetChannel::Sts).matches(&version));
        assert!(DotnetRequest::Channel(DotnetChannel::Sts).matches(&sts_version));
        assert!(DotnetRequest::Channel(DotnetChannel::Lts).matches(&legacy_lts));
        assert!(DotnetRequest::Channel(DotnetChannel::Sts).matches(&legacy_sts));
        assert!(DotnetRequest::Exact(8, 0, 100).matches(&version));
        assert!(!DotnetRequest::Exact(8, 0, 101).matches(&version));
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
        assert!(DotnetRequest::Channel(DotnetChannel::Version(8, 0)).satisfied_by(&install_info));
        assert!(
            DotnetRequest::Channel(DotnetChannel::FeatureBand(8, 0, 1)).satisfied_by(&install_info)
        );
        assert!(
            !DotnetRequest::Channel(DotnetChannel::FeatureBand(8, 0, 2))
                .satisfied_by(&install_info)
        );
        assert!(DotnetRequest::Channel(DotnetChannel::Lts).satisfied_by(&install_info));
        assert!(!DotnetRequest::Channel(DotnetChannel::Sts).satisfied_by(&install_info));
        assert!(DotnetRequest::Exact(8, 0, 100).satisfied_by(&install_info));
        assert!(!DotnetRequest::Exact(8, 0, 101).satisfied_by(&install_info));
        assert!(!DotnetRequest::Channel(DotnetChannel::Version(9, 0)).satisfied_by(&install_info));

        // Test through LanguageRequest dispatch
        let req = LanguageRequest::parse(Language::Dotnet, "8").unwrap();
        assert!(req.satisfied_by(&install_info));
        let req = LanguageRequest::parse(Language::Dotnet, "9").unwrap();
        assert!(!req.satisfied_by(&install_info));
        let req = LanguageRequest::parse(Language::Dotnet, "lts").unwrap();
        assert!(req.satisfied_by(&install_info));
        let req = LanguageRequest::parse(Language::Dotnet, "sts").unwrap();
        assert!(!req.satisfied_by(&install_info));

        Ok(())
    }
}
