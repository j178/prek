use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

use crate::hook::InstallInfo;
use crate::languages::version::{Error, try_into_u64_slice};

#[derive(Debug, Clone)]
pub(crate) enum Lts {
    NotLts,
    Codename(String),
}

impl Lts {
    pub(crate) fn code_name(&self) -> Option<&str> {
        match self {
            Self::NotLts => None,
            Self::Codename(name) => Some(name),
        }
    }
}

impl<'de> Deserialize<'de> for Lts {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::String(s) => Ok(Lts::Codename(s)),
            Value::Bool(false) => Ok(Lts::NotLts),
            Value::Null => Ok(Lts::NotLts),
            _ => Ok(Lts::NotLts),
        }
    }
}

impl Serialize for Lts {
    fn serialize<S>(&self, serializer: S) -> anyhow::Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Lts::Codename(name) => serializer.serialize_str(name),
            Lts::NotLts => serializer.serialize_bool(false),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct NodeVersion {
    pub version: semver::Version,
    pub lts: Lts,
}

impl Default for NodeVersion {
    fn default() -> Self {
        NodeVersion {
            version: semver::Version::new(0, 0, 0),
            lts: Lts::NotLts,
        }
    }
}

impl<'de> Deserialize<'de> for NodeVersion {
    fn deserialize<D>(deserializer: D) -> anyhow::Result<NodeVersion, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct _Version {
            version: String,
            lts: Lts,
        }

        let raw = _Version::deserialize(deserializer)?;
        let version_str = raw.version.trim_start_matches('v');
        let version = semver::Version::parse(version_str).map_err(serde::de::Error::custom)?;
        Ok(NodeVersion {
            version,
            lts: raw.lts,
        })
    }
}

impl Display for NodeVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.version)?;
        if let Some(name) = self.lts.code_name() {
            write!(f, "-{name}")?;
        }
        Ok(())
    }
}

impl FromStr for NodeVersion {
    type Err = semver::Error;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        // Split on the first '-' to separate version and codename
        let (version_part, lts) = match s.split_once('-') {
            Some((ver, codename)) => (ver, Lts::Codename(codename.to_string())),
            None => (s, Lts::NotLts),
        };
        let version = semver::Version::parse(version_part)?;
        Ok(NodeVersion { version, lts })
    }
}

impl NodeVersion {
    pub fn major(&self) -> u64 {
        self.version.major
    }
    pub fn minor(&self) -> u64 {
        self.version.minor
    }
    pub fn patch(&self) -> u64 {
        self.version.patch
    }
    pub fn version(&self) -> &semver::Version {
        &self.version
    }
}

/// The `language_version` field of node language, can be one of the following:
/// - `default`: Find the system installed node, or download the latest version.
/// - `system`: Find the system installed node, or return an error if not found.
/// - `x.y.z`: Install the specific version of node.
/// - `x.y`: Install the latest version of node with the same major and minor version.
/// - `x`: Install the latest version of node with the same major version.
/// - `^x.y.z`: Install the latest version of node that satisfies the version requirement.
///   Or any other semver compatible version requirement.
/// - `lts/<codename>`: Install the latest version of node with the specified code name.
/// - `local/path/to/node`: Use the node executable at the specified path.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum NodeRequest {
    Any,
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Path(PathBuf),
    Range(semver::VersionReq),
    CodeName(String),
}

impl FromStr for NodeRequest {
    type Err = Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        if request.is_empty() {
            return Ok(Self::Any);
        }

        if let Some(version_part) = request.strip_prefix("node") {
            if version_part.is_empty() {
                return Ok(Self::Any);
            }
            Self::parse_version_numbers(version_part, request)
        } else if let Some(code_name) = request.strip_prefix("lts/") {
            if code_name
                .chars()
                .all(|c| matches!(c, 'a'..='z' | 'A'..='Z' | '0'..='9'))
            {
                Ok(NodeRequest::CodeName(code_name.to_string()))
            } else {
                Err(Error::InvalidVersion(request.to_string()))
            }
        } else {
            Self::parse_version_numbers(request, request)
                .or_else(|_| {
                    semver::VersionReq::parse(request)
                        .map(NodeRequest::Range)
                        .map_err(|_| Error::InvalidVersion(request.to_string()))
                })
                .or_else(|_| {
                    let path = PathBuf::from(request);
                    if path.exists() {
                        Ok(NodeRequest::Path(path))
                    } else {
                        Err(Error::InvalidVersion(request.to_string()))
                    }
                })
        }
    }
}

pub(crate) const EXTRA_KEY_LTS: &str = "lts";

impl NodeRequest {
    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> Result<NodeRequest, Error> {
        let parts = try_into_u64_slice(version_str)
            .map_err(|_| Error::InvalidVersion(original_request.to_string()))?;

        match parts.as_slice() {
            [major] => Ok(NodeRequest::Major(*major)),
            [major, minor] => Ok(NodeRequest::MajorMinor(*major, *minor)),
            [major, minor, patch] => Ok(NodeRequest::MajorMinorPatch(*major, *minor, *patch)),
            _ => Err(Error::InvalidVersion(original_request.to_string())),
        }
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        let version = &install_info.language_version;
        let tls = install_info
            .get_extra(EXTRA_KEY_LTS)
            .and_then(|s| serde_json::from_str(s).ok())
            .unwrap_or(Lts::NotLts);

        self.matches(
            &NodeVersion {
                version: version.clone(),
                lts: tls,
            },
            Some(install_info.toolchain.as_ref()),
        )
    }

    pub(crate) fn matches(&self, version: &NodeVersion, toolchain: Option<&Path>) -> bool {
        match self {
            NodeRequest::Any => true,
            NodeRequest::Major(major) => version.major() == *major,
            NodeRequest::MajorMinor(major, minor) => {
                version.major() == *major && version.minor() == *minor
            }
            NodeRequest::MajorMinorPatch(major, minor, patch) => {
                version.major() == *major && version.minor() == *minor && version.patch() == *patch
            }
            NodeRequest::Path(path) => toolchain.is_some_and(|t| t == path),
            NodeRequest::Range(req) => req.matches(version.version()),
            NodeRequest::CodeName(name) => version
                .lts
                .code_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(name)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{EXTRA_KEY_LTS, NodeRequest};
    use crate::config::Language;
    use crate::hook::InstallInfo;
    use std::collections::HashSet;
    use std::path::{Path, PathBuf};
    use std::str::FromStr;

    #[test]
    fn test_node_request_from_str() {
        assert_eq!(NodeRequest::from_str("node").unwrap(), NodeRequest::Any);
        assert_eq!(
            NodeRequest::from_str("node12").unwrap(),
            NodeRequest::Major(12)
        );
        assert_eq!(
            NodeRequest::from_str("node12.18").unwrap(),
            NodeRequest::MajorMinor(12, 18)
        );
        assert_eq!(
            NodeRequest::from_str("node12.18.3").unwrap(),
            NodeRequest::MajorMinorPatch(12, 18, 3)
        );
        assert_eq!(
            NodeRequest::from_str("lts/Argon").unwrap(),
            NodeRequest::CodeName("Argon".to_string())
        );
        assert_eq!(NodeRequest::from_str("").unwrap(), NodeRequest::Any);
        assert_eq!(NodeRequest::from_str("12").unwrap(), NodeRequest::Major(12));
        assert_eq!(
            NodeRequest::from_str("12.18").unwrap(),
            NodeRequest::MajorMinor(12, 18)
        );
        assert_eq!(
            NodeRequest::from_str("12.18.3").unwrap(),
            NodeRequest::MajorMinorPatch(12, 18, 3)
        );
        assert_eq!(
            NodeRequest::from_str(">=12.18").unwrap(),
            NodeRequest::Range(semver::VersionReq::parse(">=12.18").unwrap())
        );
    }

    #[test]
    fn test_node_request_invalid() {
        assert!(NodeRequest::from_str("node12.18.3.4").is_err());
        assert!(NodeRequest::from_str("node12.18.3a").is_err());
        assert!(NodeRequest::from_str("node12.18.x").is_err());
        assert!(NodeRequest::from_str("node^12.18.3").is_err(),);
        assert!(NodeRequest::from_str("invalid").is_err());
        assert!(NodeRequest::from_str("lts/$$$").is_err());
    }

    #[test]
    fn test_node_request_satisfied_by() {
        let mut install_info = InstallInfo::new(Language::Node, HashSet::default(), Path::new("."));
        install_info
            .with_language_version(semver::Version::new(12, 18, 3))
            .with_toolchain(PathBuf::from("/usr/bin/node"))
            .with_extra(EXTRA_KEY_LTS, "\"Argon\"");

        let request = NodeRequest::Major(12);
        assert!(request.satisfied_by(&install_info));

        let request = NodeRequest::MajorMinor(12, 18);
        assert!(request.satisfied_by(&install_info));

        let request = NodeRequest::MajorMinorPatch(12, 18, 3);
        assert!(request.satisfied_by(&install_info));

        let request = NodeRequest::CodeName("Argon".to_string());
        assert!(request.satisfied_by(&install_info));

        let request = NodeRequest::CodeName("argon".to_string());
        assert!(request.satisfied_by(&install_info));

        let request = NodeRequest::CodeName("Boron".to_string());
        assert!(!request.satisfied_by(&install_info));

        let request = NodeRequest::Path(PathBuf::from("/usr/bin/node"));
        assert!(request.satisfied_by(&install_info));

        let request = NodeRequest::Path(PathBuf::from("/usr/bin/nodejs"));
        assert!(!request.satisfied_by(&install_info));

        let request = NodeRequest::Range(semver::VersionReq::parse(">=12.18").unwrap());
        assert!(request.satisfied_by(&install_info));

        let request = NodeRequest::Range(semver::VersionReq::parse(">=13.0").unwrap());
        assert!(!request.satisfied_by(&install_info));
    }
}
