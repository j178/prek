#![warn(dead_code)]
#![warn(clippy::missing_errors_doc)]
#![warn(clippy::missing_panics_doc)]
#![warn(clippy::must_use_candidate)]
#![warn(clippy::module_name_repetitions)]
#![warn(clippy::too_many_arguments)]

use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::hook::InstallInfo;
use crate::languages::version::Error;

/// Ruby version request parsed from `language_version` field
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum RubyRequest {
    /// Any available Ruby (prefer system, then latest)
    Any,

    /// Exact major.minor.patch version
    Exact(u64, u64, u64),

    /// Major.minor (latest patch)
    MajorMinor(u64, u64),

    /// Major version (latest minor.patch)
    Major(u64),

    /// Explicit file path to Ruby interpreter
    Path(PathBuf),

    /// Semver range (e.g., ">=3.2, <4.0")
    Range(semver::VersionReq, String),
}

impl FromStr for RubyRequest {
    type Err = Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        // Empty/default
        if s.is_empty() || s == "default" {
            return Ok(Self::Any);
        }

        // "system" specified explicitly
        if s == "system" {
            return Ok(Self::Any);
        }

        // Strip "ruby-" prefix if present
        let version_part = s.strip_prefix("ruby").unwrap_or(s);
        let version_part = version_part.strip_prefix('-').unwrap_or(version_part);

        // Try parsing as version numbers (any of one to three parts)
        if let Ok(parts) = parse_version_parts(version_part) {
            return Ok(match parts.as_slice() {
                [major] => Self::Major(*major),
                [major, minor] => Self::MajorMinor(*major, *minor),
                [major, minor, patch] => Self::Exact(*major, *minor, *patch),
                _ => return Err(Error::InvalidVersion(s.to_string())),
            });
        }

        // Try parsing as semver range
        if let Ok(req) = semver::VersionReq::parse(s) {
            return Ok(Self::Range(req, s.to_string()));
        }

        // Finally try as a file path
        let path = PathBuf::from(s);
        if path.exists() {
            return Ok(Self::Path(path));
        }

        Err(Error::InvalidVersion(s.to_string()))
    }
}

impl RubyRequest {
    /// Check if this request accepts any Ruby version
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }

    /// Check if this request matches a Ruby version during installation search
    ///
    /// This is used by the installer when searching for existing Ruby installations.
    pub(crate) fn matches(&self, version: &semver::Version, toolchain: Option<&Path>) -> bool {
        match self {
            Self::Any => true,
            Self::Exact(maj, min, patch) => {
                version.major == *maj && version.minor == *min && version.patch == *patch
            }
            Self::MajorMinor(maj, min) => version.major == *maj && version.minor == *min,
            Self::Major(maj) => version.major == *maj,
            Self::Path(path) => toolchain.is_some_and(|t| t == path),
            Self::Range(req, _) => req.matches(version),
        }
    }

    /// Check if this request is satisfied by the given Ruby installation
    ///
    /// This is used at runtime to verify an installation meets the requirements.
    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        self.matches(
            &install_info.language_version,
            Some(&install_info.toolchain),
        )
    }
}

fn parse_version_parts(s: &str) -> Result<Vec<u64>, std::num::ParseIntError> {
    s.split('.').map(str::parse::<u64>).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_ruby_request() {
        // Empty/default
        assert_eq!(RubyRequest::from_str("").unwrap(), RubyRequest::Any);
        assert_eq!(RubyRequest::from_str("default").unwrap(), RubyRequest::Any);
        assert_eq!(RubyRequest::from_str("system").unwrap(), RubyRequest::Any);

        // Exact versions
        assert_eq!(
            RubyRequest::from_str("3.3.6").unwrap(),
            RubyRequest::Exact(3, 3, 6)
        );
        assert_eq!(
            RubyRequest::from_str("ruby-3.3.6").unwrap(),
            RubyRequest::Exact(3, 3, 6)
        );

        // Major.minor
        assert_eq!(
            RubyRequest::from_str("3.3").unwrap(),
            RubyRequest::MajorMinor(3, 3)
        );
        assert_eq!(
            RubyRequest::from_str("ruby-3.3").unwrap(),
            RubyRequest::MajorMinor(3, 3)
        );

        // Major only
        assert_eq!(RubyRequest::from_str("3").unwrap(), RubyRequest::Major(3));
        assert_eq!(
            RubyRequest::from_str("ruby-3").unwrap(),
            RubyRequest::Major(3)
        );

        // Semver range
        assert!(matches!(
            RubyRequest::from_str(">=3.2, <4.0").unwrap(),
            RubyRequest::Range(_, _)
        ));
    }

    #[test]
    fn test_version_matching() -> anyhow::Result<()> {
        use std::path::Path;

        use crate::config::Language;
        use rustc_hash::FxHashSet;

        let mut install_info =
            InstallInfo::new(Language::Ruby, FxHashSet::default(), Path::new("."))?;
        install_info
            .with_language_version(semver::Version::new(3, 3, 6))
            .with_toolchain(PathBuf::from("/usr/bin/ruby"));

        assert!(RubyRequest::Any.satisfied_by(&install_info));
        assert!(RubyRequest::Exact(3, 3, 6).satisfied_by(&install_info));
        assert!(RubyRequest::MajorMinor(3, 3).satisfied_by(&install_info));
        assert!(RubyRequest::Major(3).satisfied_by(&install_info));
        assert!(!RubyRequest::Exact(3, 3, 7).satisfied_by(&install_info));
        assert!(!RubyRequest::Exact(3, 2, 6).satisfied_by(&install_info));

        // Test path matching
        assert!(RubyRequest::Path(PathBuf::from("/usr/bin/ruby")).satisfied_by(&install_info));
        assert!(!RubyRequest::Path(PathBuf::from("/usr/bin/ruby3.2")).satisfied_by(&install_info));

        // Test range matching
        let req = semver::VersionReq::parse(">=3.2, <4.0").unwrap();
        assert!(
            RubyRequest::Range(req.clone(), ">=3.2, <4.0".to_string()).satisfied_by(&install_info)
        );

        let mut install_info_old =
            InstallInfo::new(Language::Ruby, FxHashSet::default(), Path::new("."))?;
        install_info_old
            .with_language_version(semver::Version::new(3, 1, 0))
            .with_toolchain(PathBuf::from("/usr/bin/ruby3.1"));
        assert!(
            !RubyRequest::Range(req, ">=3.2, <4.0".to_string()).satisfied_by(&install_info_old)
        );

        Ok(())
    }
}
