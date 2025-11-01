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
        let version = &install_info.language_version;

        self.matches(
            &RustVersion(version.clone()),
            Some(install_info.toolchain.as_ref()),
        )
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
