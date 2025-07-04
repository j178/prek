use std::borrow::Cow;
use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::string::ToString;

use anyhow::Result;
use futures::TryStreamExt;
use itertools::Itertools;
use reqwest::Client;
use serde::Deserialize;
use target_lexicon::{Architecture, HOST, OperatingSystem, X86_32Architecture};
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{trace, warn};

use crate::archive;
use crate::archive::ArchiveExtension;
use crate::config::LanguageVersion;
use crate::fs::LockedFile;

#[derive(Deserialize, Debug)]
#[serde(untagged)]
pub enum Lts {
    Boolean(bool),
    CodeName(String),
}

impl Lts {
    pub fn is_lts(&self) -> bool {
        match self {
            Lts::Boolean(b) => *b,
            Lts::CodeName(_) => true,
        }
    }

    pub fn code_name(&self) -> Option<&str> {
        match self {
            Lts::Boolean(_) => None,
            Lts::CodeName(name) => Some(name),
        }
    }
}

#[derive(Debug)]
pub struct NodeVersion {
    pub version: semver::Version,
    pub lts: Lts,
}

impl FromStr for NodeVersion {
    type Err = semver::Error;

    /// Parse from `<version>[-<code_name>]` format.
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let (ver, code_name) = if s.contains('-') {
            s.split_once('-').unwrap()
        } else {
            (s, "")
        };
        let version = semver::Version::parse(ver)?;
        let lts = if code_name.is_empty() {
            Lts::Boolean(false)
        } else {
            Lts::CodeName(code_name.to_string())
        };
        Ok(NodeVersion { version, lts })
    }
}

impl<'de> Deserialize<'de> for NodeVersion {
    fn deserialize<D>(deserializer: D) -> Result<NodeVersion, D::Error>
    where
        D: serde::Deserializer<'de>,
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
        if let Lts::CodeName(name) = &self.lts {
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

// TODO: remove code name support
pub enum VersionRequest {
    Any,
    None,
    // TODO: remove, just use semver::VersionReq
    Major(u64),
    MajorMinor(u64, u64),
    MajorMinorPatch(u64, u64, u64),
    Range(semver::VersionReq),
    CodeName(String),
}

impl VersionRequest {
    pub fn matches(&self, version: &NodeVersion) -> bool {
        match self {
            VersionRequest::Any => true,
            VersionRequest::None => false,
            VersionRequest::Major(major) => version.major() == *major,
            VersionRequest::MajorMinor(major, minor) => {
                version.major() == *major && version.minor() == *minor
            }
            VersionRequest::MajorMinorPatch(major, minor, patch) => {
                (version.major(), version.minor(), version.patch()) == (*major, *minor, *patch)
            }
            VersionRequest::Range(req) => req.matches(version.version()),
            VersionRequest::CodeName(name) => version
                .lts
                .code_name()
                .is_some_and(|n| n.eq_ignore_ascii_case(name)),
        }
    }
}

impl FromStr for VersionRequest {
    type Err = semver::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let parts = s
            .split('.')
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>();
        if let Ok(parts) = parts {
            match parts.as_slice() {
                [major] => return Ok(VersionRequest::Major(*major)),
                [major, minor] => return Ok(VersionRequest::MajorMinor(*major, *minor)),
                [major, minor, patch] => {
                    return Ok(VersionRequest::MajorMinorPatch(*major, *minor, *patch));
                }
                _ => {}
            }
        }

        if let Ok(req) = semver::VersionReq::parse(s) {
            return Ok(VersionRequest::Range(req));
        }

        Ok(VersionRequest::CodeName(s.to_string()))
    }
}

impl TryFrom<LanguageVersion> for VersionRequest {
    type Error = <Self as FromStr>::Err;

    fn try_from(_version: LanguageVersion) -> Result<Self, Self::Error> {
        todo!()
    }
}

#[derive(Debug)]
pub struct NodeResult {
    node: Option<PathBuf>,
    npm: Option<PathBuf>,
    dir: Option<PathBuf>,
}

impl Display for NodeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{}",
            self.node.as_ref().or(self.dir.as_ref()).unwrap().display()
        )?;
        Ok(())
    }
}

impl NodeResult {
    pub fn from_executables(node: PathBuf, npm: PathBuf) -> Self {
        Self {
            node: Some(node),
            npm: Some(npm),
            dir: None,
        }
    }

    pub fn from_dir(dir: PathBuf) -> Self {
        Self {
            node: None,
            npm: None,
            dir: Some(dir),
        }
    }

    pub fn node(&self) -> Cow<Path> {
        match &self.node {
            Some(path) => Cow::Borrowed(path),
            None => Cow::Owned(
                bin_dir(self.dir.as_ref().unwrap())
                    .join("node")
                    .with_extension(EXE_EXTENSION),
            ),
        }
    }

    pub fn npm(&self) -> Cow<Path> {
        match &self.npm {
            Some(path) => Cow::Borrowed(path),
            None => Cow::Owned(
                bin_dir(self.dir.as_ref().unwrap())
                    .join("npm")
                    .with_extension(EXE_EXTENSION),
            ),
        }
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
pub struct NodeInstaller {
    root: PathBuf,
    client: Client,
}

impl NodeInstaller {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root,
            client: Client::new(),
        }
    }

    /// Install a version of Node.js.
    pub async fn install(&self, version: &LanguageVersion) -> Result<NodeResult> {
        if version.allow_system() {
            let node = which::which("node");
            let npm = which::which("npm");
            if let (Ok(node), Ok(npm)) = (node, npm) {
                trace!(node = %node.display(), npm = %npm.display(), "Found system node and npm");
                return Ok(NodeResult::from_executables(node, npm));
            }
        }
        if !version.allow_managed() {
            return Err(anyhow::anyhow!(
                "Node not found on the system and downloading is disabled"
            ));
        }

        fs_err::create_dir_all(&self.root)?;

        let version_req = VersionRequest::try_from(version.clone())?;
        if let Ok(node) = self.get_installed(&version_req) {
            trace!(%node, "Found installed node");
            return Ok(node);
        }

        let _lock = LockedFile::acquire(self.root.join(".lock"), "node").await?;

        if let Ok(node) = self.get_installed(&version_req) {
            trace!(%node, "Found installed node");
            return Ok(node);
        }

        let resolved_version = self.resolve_version(&version_req).await?;
        trace!(version = %resolved_version, "Installing node");

        self.install_node(&resolved_version).await
    }

    /// Get the installed version of Node.js.
    fn get_installed(&self, req: &VersionRequest) -> Result<NodeResult> {
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
                let version = NodeVersion::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| a.version.cmp(&b.version))
            .rev();

        installed
            .find_map(|(v, path)| {
                if req.matches(&v) {
                    Some(NodeResult::from_dir(path))
                } else {
                    None
                }
            })
            .ok_or(anyhow::anyhow!("No installed node found"))
    }

    async fn resolve_version(&self, req: &VersionRequest) -> Result<NodeVersion> {
        let versions = self.list_remote_versions().await?;
        let version = versions
            .into_iter()
            .find(|version| req.matches(version))
            .ok_or(anyhow::anyhow!("Version not found"))?;
        Ok(version)
    }

    /// List all versions of Node.js available on the Node.js website.
    async fn list_remote_versions(&self) -> Result<Vec<NodeVersion>> {
        let url = "https://nodejs.org/dist/index.json";
        let versions: Vec<NodeVersion> = self.client.get(url).send().await?.json().await?;
        Ok(versions)
    }

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
        fs_err::tokio::rename(extracted, &target).await?;

        Ok(NodeResult::from_dir(target))
    }
}

#[cfg(not(windows))]
fn bin_dir(root: &Path) -> PathBuf {
    root.join("bin")
}

#[cfg(windows)]
fn bin_dir(root: &Path) -> PathBuf {
    root.to_path_buf()
}
