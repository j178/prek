use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::string::ToString;

use anyhow::Result;
use futures::TryStreamExt;
use itertools::Itertools;
use reqwest::Client;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use target_lexicon::{Architecture, HOST, OperatingSystem, X86_32Architecture};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{trace, warn};

use crate::archive;
use crate::archive::ArchiveExtension;
use crate::fs::LockedFile;
use crate::hook::InstallInfo;
use crate::languages::version::{Error, LanguageRequest, try_into_u8_slice};
use crate::process::Cmd;

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
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
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
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
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
    fn deserialize<D>(deserializer: D) -> Result<NodeVersion, D::Error>
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum NodeRequest {
    Major(u8),
    MajorMinor(u8, u8),
    MajorMinorPatch(u8, u8, u8),
    Path(PathBuf),
    Range(semver::VersionReq),
    CodeName(String),
}

pub(crate) const EXTRA_KEY_LTS: &str = "lts";

impl NodeRequest {
    pub(crate) fn parse(request: &str) -> std::result::Result<LanguageRequest, Error> {
        if request.is_empty() {
            return Ok(LanguageRequest::Any);
        }

        let request = if let Some(version_part) = request.strip_prefix("node") {
            if version_part.is_empty() {
                return Ok(LanguageRequest::Any);
            }
            Self::parse_version_numbers(version_part, request)
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
                .or_else(|_| {
                    // If it doesn't match any known format, treat it as a code name
                    Ok(NodeRequest::CodeName(request.to_string()))
                })
        };

        Ok(LanguageRequest::Node(request?))
    }

    fn parse_version_numbers(
        version_str: &str,
        original_request: &str,
    ) -> std::result::Result<NodeRequest, Error> {
        let parts = try_into_u8_slice(version_str)
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

        self.matches(&NodeVersion {
            version: version.clone(),
            lts: tls,
        })
    }

    pub(crate) fn matches(&self, version: &NodeVersion) -> bool {
        match self {
            NodeRequest::Major(major) => version.major() == u64::from(*major),
            NodeRequest::MajorMinor(major, minor) => {
                version.major() == u64::from(*major) && version.minor() == u64::from(*minor)
            }
            NodeRequest::MajorMinorPatch(major, minor, patch) => {
                version.major() == u64::from(*major)
                    && version.minor() == u64::from(*minor)
                    && version.patch() == u64::from(*patch)
            }
            NodeRequest::Path(path) => path.exists(),
            NodeRequest::Range(req) => req.matches(version.version()),
            NodeRequest::CodeName(name) => version
                .lts
                .code_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(name)),
        }
    }
}

#[derive(Debug)]
pub(crate) struct NodeResult {
    node: PathBuf,
    npm: PathBuf,
    version: NodeVersion,
}

impl Display for NodeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.node.display())?;
        Ok(())
    }
}

impl NodeResult {
    pub(crate) fn from_executables(node: PathBuf, npm: PathBuf) -> Self {
        Self {
            node,
            npm,
            version: NodeVersion::default(),
        }
    }

    pub(crate) fn from_dir(dir: &Path) -> Self {
        let node = bin_dir(dir).join("node").with_extension(EXE_EXTENSION);
        let npm = bin_dir(dir).join("npm").with_extension(EXE_EXTENSION);
        Self::from_executables(node, npm)
    }

    pub(crate) fn with_version(mut self, version: NodeVersion) -> Self {
        self.version = version;
        self
    }

    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        // https://nodejs.org/api/process.html#processrelease
        let output = Cmd::new(&self.node, "node -p")
            .arg("-p")
            .arg("JSON.stringify({version: process.version, lts: process.release.lts || false})")
            .check(true)
            .output()
            .await?;
        let output_str = String::from_utf8_lossy(&output.stdout);
        let version: NodeVersion = serde_json::from_str(&output_str)
            .map_err(|e| anyhow::anyhow!("Failed to parse node version: {}", e))?;

        self.version = version;

        Ok(self)
    }

    pub(crate) fn node(&self) -> &Path {
        &self.node
    }

    pub(crate) fn npm(&self) -> &Path {
        &self.npm
    }

    pub(crate) fn version(&self) -> &NodeVersion {
        &self.version
    }
}

/// A Node.js installer.
/// The `language_version` field of node language, can be one of the following:
/// - `default`: Find the system installed node, or download the latest version.
/// - `system`: Find the system installed node, or return an error if not found.
/// - `x.y.z`: Install the specific version of node.
/// - `x.y`: Install the latest version of node with the same major and minor version.
/// - `x`: Install the latest version of node with the same major version.
/// - `^x.y.z`: Install the latest version of node that satisfies the version requirement.
///   Or any other semver compatible version requirement.
/// - `codename`: Install the latest version of node with the specified code name.
pub(crate) struct NodeInstaller {
    root: PathBuf,
    client: Client,
}

impl NodeInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: Client::new(),
        }
    }

    /// Install a version of Node.js.
    pub(crate) async fn install(&self, request: &LanguageRequest) -> Result<NodeResult> {
        fs_err::create_dir_all(&self.root)?;

        let _lock = LockedFile::acquire(self.root.join(".lock"), "node").await?;

        let node_request = match request {
            LanguageRequest::Any => None,
            LanguageRequest::Node(request) => Some(request),
            _ => unreachable!(),
        };
        if let Ok(node) = self.find_installed(node_request) {
            trace!(%node, "Found installed node");
            return Ok(node);
        }

        let node = which::which("node");
        let npm = which::which("npm");
        if let (Ok(node), Ok(npm)) = (node, npm) {
            trace!(node = %node.display(), npm = %npm.display(), "Found system node and npm");
            return NodeResult::from_executables(node, npm).fill_version().await;
        }

        let resolved_version = self.resolve_version(node_request).await?;
        trace!(version = %resolved_version, "Installing node");

        self.install_node(&resolved_version).await
    }

    /// Get the installed version of Node.js.
    fn find_installed(&self, req: Option<&NodeRequest>) -> Result<NodeResult> {
        let mut installed = fs_err::read_dir(&self.root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(err) => {
                    warn!(?err, "Failed to read entry");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_ok_and(|f| f.is_dir()))
            .filter_map(|entry| {
                let dir_name = entry.file_name();
                let version: NodeVersion =
                    serde_json::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| a.version.cmp(&b.version))
            .rev();

        installed
            .find_map(|(v, path)| {
                if req.is_none_or(|req| req.matches(&v)) {
                    Some(NodeResult::from_dir(&path).with_version(v))
                } else {
                    None
                }
            })
            .ok_or(anyhow::anyhow!("No installed node found"))
    }

    async fn resolve_version(&self, req: Option<&NodeRequest>) -> Result<NodeVersion> {
        let versions = self.list_remote_versions().await?;
        let version = versions
            .into_iter()
            .find(|version| req.is_none_or(|req| req.matches(version)))
            .ok_or(anyhow::anyhow!("Version not found"))?;
        Ok(version)
    }

    /// List all versions of Node.js available on the Node.js website.
    async fn list_remote_versions(&self) -> Result<Vec<NodeVersion>> {
        let url = "https://nodejs.org/dist/index.json";
        let versions: Vec<NodeVersion> = self.client.get(url).send().await?.json().await?;
        Ok(versions)
    }

    // TODO: support mirror?
    /// Install a specific version of Node.js.
    async fn install_node(&self, version: &NodeVersion) -> Result<NodeResult> {
        let arch = match HOST.architecture {
            Architecture::X86_32(X86_32Architecture::I686) => "x86",
            Architecture::X86_64 => "x64",
            Architecture::Aarch64(_) => "arm64",
            Architecture::Arm(_) => "armv7l",
            Architecture::S390x => "s390x",
            Architecture::Powerpc => "ppc64",
            Architecture::Powerpc64le => "ppc64le",
            _ => return Err(anyhow::anyhow!("Unsupported architecture")),
        };
        let os = match HOST.operating_system {
            OperatingSystem::Darwin(_) => "darwin",
            OperatingSystem::Linux => "linux",
            OperatingSystem::Windows => "win",
            OperatingSystem::Aix => "aix",
            _ => return Err(anyhow::anyhow!("Unsupported OS")),
        };
        let ext = if cfg!(windows) { "zip" } else { "tar.xz" };

        let filename = format!("node-v{}-{os}-{arch}.{ext}", version.version());
        let url = format!("https://nodejs.org/dist/v{}/{filename}", version.version());
        let target = self.root.join(version.to_string());

        let tarball = self
            .client
            .get(&url)
            .send()
            .await?
            .bytes_stream()
            .map_err(std::io::Error::other)
            .into_async_read()
            .compat();

        let temp_dir = tempfile::tempdir_in(&self.root)?;
        trace!(url = %url, temp_dir = ?temp_dir.path(), "Downloading node");

        let ext = ArchiveExtension::from_path(&filename)?;
        archive::unpack(tarball, ext, temp_dir.path()).await?;

        let extracted = match archive::strip_component(temp_dir.path()) {
            Ok(top_level) => top_level,
            Err(archive::Error::NonSingularArchive(_)) => temp_dir.keep(),
            Err(err) => return Err(err.into()),
        };

        if target.is_dir() {
            trace!(target = %target.display(), "Removing existing target");
            fs_err::tokio::remove_dir_all(&target).await?;
        }

        trace!(temp_dir = ?extracted, target = %target.display(), "Moving node to target");
        // TODO: retry on Windows
        fs_err::tokio::rename(extracted, &target).await?;

        Ok(NodeResult::from_dir(&target).with_version(version.clone()))
    }
}

#[cfg(not(windows))]
pub(crate) fn bin_dir(root: &Path) -> PathBuf {
    root.join("bin")
}

#[cfg(windows)]
pub(crate) fn bin_dir(root: &Path) -> PathBuf {
    root.to_path_buf()
}
