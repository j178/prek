use std::fmt::Display;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::Deserialize;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DenoVersion(semver::Version);

impl Default for DenoVersion {
    fn default() -> Self {
        DenoVersion(semver::Version::new(0, 0, 0))
    }
}

impl Deref for DenoVersion {
    type Target = semver::Version;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Display for DenoVersion {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.0)
    }
}

impl<'de> Deserialize<'de> for DenoVersion {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let version_str = String::deserialize(deserializer)?;
        version_str.parse().map_err(serde::de::Error::custom)
    }
}

impl FromStr for DenoVersion {
    type Err = semver::Error;

    fn from_str(string: &str) -> Result<Self, Self::Err> {
        let string = string.strip_prefix('v').unwrap_or(string).trim();
        semver::Version::parse(string).map(DenoVersion)
    }
}

/// `language_version` field of deno can be one of the following:
/// - `default`: Find system installed deno, or download the latest version
/// - `system`: Find system installed deno, or error if not found
/// - `deno` or `deno@latest`: Same as `default`
/// - `1.40` or `deno@1.40`: Install latest 1.40.x version
/// - `1.40.0` or `deno@1.40.0`: Install specific version
/// - `>= 1.40, < 1.50`: Install latest version matching semver range
/// - `local/path/to/deno`: Use deno executable at the specified path
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum DenoRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Path(PathBuf),
    Range(semver::VersionReq, String),
}

impl FromStr for DenoRequest {
    type Err = Error;

    fn from_str(string: &str) -> Result<Self, Self::Err> {
        if string.is_empty() {
            return Ok(DenoRequest::Any);
        }

        // Handle "deno" or "deno@version" format
        let version_part = string
            .strip_prefix("deno@")
            .or_else(|| {
                string
                    .strip_prefix("deno")
                    .and_then(|rest| if rest.is_empty() { None } else { Some(rest) })
            })
            .unwrap_or(string);

        if string.starts_with("deno") && version_part == string {
            return Ok(DenoRequest::Any);
        }

        // Handle "latest" keyword
        if version_part.eq_ignore_ascii_case("latest") {
            return Ok(DenoRequest::Any);
        }

        Self::parse_version_numbers(version_part, string)
            .or_else(|_| {
                semver::VersionReq::parse(version_part)
                    .map(|version_req| DenoRequest::Range(version_req, string.into()))
                    .map_err(|_| Error::InvalidVersion(string.to_string()))
            })
            .or_else(|_| {
                let path = PathBuf::from(string);
                if path.exists() {
                    Ok(DenoRequest::Path(path))
                } else {
                    Err(Error::InvalidVersion(string.to_string()))
                }
            })
    }
}

impl DenoRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, DenoRequest::Any)
    }

    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> Result<DenoRequest, Error> {
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(original_request.to_string()))?;

        match parts.as_slice() {
            [major] => Ok(DenoRequest::Major(*major)),
            [major, minor] => Ok(DenoRequest::MajorMinor(*major, *minor)),
            [major, minor, patch] => Ok(DenoRequest::MajorMinorPatch(*major, *minor, *patch)),
            _ => Err(Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = &install_info.language_version;

        self.matches(
            &DenoVersion(version.clone()),
            Some(install_info.toolchain.as_ref()),
        )
    }

    pub(crate) fn matches(&self, version: &DenoVersion, toolchain: Option<&Path>) -> bool {
        match self {
            Self::Any => true,
            Self::Major(major) => version.major == *major,
            Self::MajorMinor(major, minor) => version.major == *major && version.minor == *minor,
            Self::MajorMinorPatch(major, minor, patch) => {
                version.major == *major && version.minor == *minor && version.patch == *patch
            }
            Self::Path(path) => toolchain.is_some_and(|toolchain_path| toolchain_path == path),
            Self::Range(req, _) => req.matches(version),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::DenoRequest;
    use std::str::FromStr;

    #[test]
    fn test_deno_request_from_str() {
        assert_eq!(DenoRequest::from_str("deno").unwrap(), DenoRequest::Any);
        assert_eq!(
            DenoRequest::from_str("deno@1").unwrap(),
            DenoRequest::Major(1)
        );
        assert_eq!(
            DenoRequest::from_str("deno@1.40").unwrap(),
            DenoRequest::MajorMinor(1, 40)
        );
        assert_eq!(
            DenoRequest::from_str("deno@1.40.0").unwrap(),
            DenoRequest::MajorMinorPatch(1, 40, 0)
        );
        assert_eq!(
            DenoRequest::from_str("deno@latest").unwrap(),
            DenoRequest::Any
        );
        assert_eq!(DenoRequest::from_str("").unwrap(), DenoRequest::Any);
        assert_eq!(DenoRequest::from_str("1").unwrap(), DenoRequest::Major(1));
        assert_eq!(
            DenoRequest::from_str("1.40").unwrap(),
            DenoRequest::MajorMinor(1, 40)
        );
        assert_eq!(
            DenoRequest::from_str("1.40.0").unwrap(),
            DenoRequest::MajorMinorPatch(1, 40, 0)
        );
    }

    #[test]
    fn test_deno_request_invalid() {
        assert!(DenoRequest::from_str("deno@1.40.0.1").is_err());
        assert!(DenoRequest::from_str("deno@1.40a").is_err());
        assert!(DenoRequest::from_str("invalid").is_err());
    }
}
