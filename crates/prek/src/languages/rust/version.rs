use std::fmt::Display;
use std::str::FromStr;

use semver::Version;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Copy, Clone, Eq, PartialEq, strum::Display, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum Channel {
    Stable,
    Beta,
    Nightly,
}

impl Channel {
    pub(crate) fn from_version(version: &Version) -> Option<Self> {
        match version.pre.as_str().split('.').next()? {
            "" => Some(Self::Stable),
            "beta" => Some(Self::Beta),
            "nightly" => Some(Self::Nightly),
            _ => None,
        }
    }
}

/// A parsed Rust version request:
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
        self.matches(&install_info.language_version)
    }

    pub(crate) fn matches(&self, version: &Version) -> bool {
        match self {
            RustRequest::Any => version.major > 0,
            RustRequest::Channel(requested_channel) => {
                Channel::from_version(version).as_ref() == Some(requested_channel)
            }
            RustRequest::Major(major) => version.major == *major,
            RustRequest::MajorMinor(major, minor) => {
                version.major == *major && version.minor == *minor
            }
            RustRequest::MajorMinorPatch(major, minor, patch) => {
                version.major == *major && version.minor == *minor && version.patch == *patch
            }
            RustRequest::Range(req, _) => req.matches(version),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Language;
    use crate::hook::InstallInfo;
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
        let version = Version::new(1, 71, 0);
        let other_version = Version::new(1, 72, 1);

        assert!(RustRequest::Any.matches(&version));
        assert!(RustRequest::Channel(Channel::Stable).matches(&version));
        assert!(RustRequest::Channel(Channel::Stable).matches(&other_version));
        assert!(RustRequest::Major(1).matches(&version));
        assert!(!RustRequest::Major(2).matches(&version));
        assert!(RustRequest::MajorMinor(1, 71).matches(&version));
        assert!(!RustRequest::MajorMinor(1, 72).matches(&version));
        assert!(RustRequest::MajorMinorPatch(1, 71, 0).matches(&version));
        assert!(!RustRequest::MajorMinorPatch(1, 71, 1).matches(&version));

        let req = semver::VersionReq::parse(">=1.70, <1.72")?;
        assert!(RustRequest::Range(req.clone(), ">=1.70, <1.72".into()).matches(&version));
        assert!(!RustRequest::Range(req, ">=1.70, <1.72".into()).matches(&other_version));

        Ok(())
    }

    #[test]
    fn channels_match_compiler_releases_and_cached_versions() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        for (release, expected) in [
            ("1.95.0", Some(Channel::Stable)),
            ("1.96.0-beta.2", Some(Channel::Beta)),
            ("1.97.0-nightly", Some(Channel::Nightly)),
            ("1.97.0-dev", None),
        ] {
            let version = Version::parse(release)?;
            let mut info = InstallInfo::create(Language::Rust, None, Vec::new(), temp_dir.path())?;
            // Legacy channel metadata must not override the compiler's release.
            info.with_language_version(version.clone())
                .with_extra("channel", "nightly");
            for channel in [Channel::Stable, Channel::Beta, Channel::Nightly] {
                let request = RustRequest::Channel(channel);
                assert_eq!(
                    request.matches(&version),
                    Some(channel) == expected,
                    "{release}: {channel}"
                );
                assert_eq!(request.satisfied_by(&info), request.matches(&version));
            }
        }
        Ok(())
    }
}
