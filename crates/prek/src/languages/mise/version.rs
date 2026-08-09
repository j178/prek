use std::str::FromStr;

use semver::{Version, VersionReq};

use crate::hook::InstallInfo;
use crate::languages::version::Error;

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum MiseRequest {
    Any,
    Range(VersionReq),
}

impl FromStr for MiseRequest {
    type Err = Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        if request.is_empty() {
            return Ok(Self::Any);
        }

        VersionReq::parse(request)
            .map(Self::Range)
            .map_err(|_| Error::InvalidVersion(request.to_string()))
    }
}

impl MiseRequest {
    pub(crate) fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }

    pub(crate) fn matches(&self, version: &Version) -> bool {
        match self {
            Self::Any => true,
            Self::Range(request) => request.matches(version),
        }
    }

    pub(crate) fn satisfied_by(&self, info: &InstallInfo) -> bool {
        self.matches(&info.language_version)
    }
}

#[cfg(test)]
mod tests {
    use super::MiseRequest;

    #[test]
    fn exact_version_requires_equals() {
        let exact: MiseRequest = "=2026.7.18".parse().unwrap();
        let compatible: MiseRequest = "2026.7.18".parse().unwrap();

        assert!(exact.matches(&"2026.7.18".parse().unwrap()));
        assert!(!exact.matches(&"2026.8.2".parse().unwrap()));
        assert!(compatible.matches(&"2026.8.2".parse().unwrap()));
    }
}
