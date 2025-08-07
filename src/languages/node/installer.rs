use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::string::ToString;

use anyhow::{Context, Result};
use itertools::Itertools;
use reqwest::Client;
use target_lexicon::{Architecture, HOST, OperatingSystem};
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::languages::download_and_extract;
use crate::languages::node::NodeRequest;
use crate::languages::node::version::NodeVersion;
use crate::process::Cmd;

#[derive(Debug)]
pub(crate) struct NodeResult {
    node: PathBuf,
    npm: PathBuf,
    version: NodeVersion,
}

impl Display for NodeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.node.display(), self.version)?;
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
        let npm = bin_dir(dir)
            .join("npm")
            .with_extension(if cfg!(windows) { "cmd" } else { "" });
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
        let version: NodeVersion =
            serde_json::from_str(&output_str).context("Failed to parse node version")?;

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
    pub(crate) async fn install(&self, request: &NodeRequest) -> Result<NodeResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;

        let _lock = LockedFile::acquire(self.root.join(".lock"), "node").await?;

        if let Ok(node_result) = self.find_installed(request) {
            trace!(%node_result, "Found installed node");
            return Ok(node_result);
        }

        // Find all node and npm executables in PATH and check their versions
        if let Some(node_result) = self.find_system_node(request).await? {
            trace!(%node_result, "Using system node");
            return Ok(node_result);
        }

        let resolved_version = self.resolve_version(request).await?;
        trace!(version = %resolved_version, "Installing node");

        self.download(&resolved_version).await
    }

    /// Get the installed version of Node.js.
    fn find_installed(&self, req: &NodeRequest) -> Result<NodeResult> {
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
                if req.matches(&v, Some(&path)) {
                    Some(NodeResult::from_dir(&path).with_version(v))
                } else {
                    None
                }
            })
            .context("No installed node version matches the request")
    }

    async fn resolve_version(&self, req: &NodeRequest) -> Result<NodeVersion> {
        let versions = self
            .list_remote_versions()
            .await
            .context("Failed to list remote versions")?;
        let version = versions
            .into_iter()
            .find(|version| req.matches(version, None))
            .context("Version not found on remote")?;
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
    async fn download(&self, version: &NodeVersion) -> Result<NodeResult> {
        let mut arch = match HOST.architecture {
            Architecture::X86_32(_) => "x86",
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
        if os == "darwin" && arch == "arm64" && version.major() < 16 {
            // Node.js 16 and later are required for arm64 on macOS.
            arch = "x64";
        }
        let ext = if cfg!(windows) { "zip" } else { "tar.xz" };

        let filename = format!("node-v{}-{os}-{arch}.{ext}", version.version());
        let url = format!("https://nodejs.org/dist/v{}/{filename}", version.version());
        let target = self.root.join(version.to_string());

        download_and_extract(&self.client, &url, &target, &filename, &self.root)
            .await
            .context("Failed to download and extract Node.js")?;

        Ok(NodeResult::from_dir(&target).with_version(version.clone()))
    }

    /// Find a suitable system Node.js installation that matches the request.
    async fn find_system_node(&self, node_request: &NodeRequest) -> Result<Option<NodeResult>> {
        let node_paths = match which::which_all("node") {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No node executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        // Check each node executable for a matching version, stop early if found
        for node_path in node_paths {
            if let Some(npm_path) = Self::find_npm_in_same_directory(&node_path)? {
                match NodeResult::from_executables(node_path, npm_path)
                    .fill_version()
                    .await
                {
                    Ok(node_result) => {
                        // Check if this version matches the request
                        if node_request.matches(&node_result.version, Some(&node_result.node)) {
                            trace!(
                                %node_result,
                                "Found a matching system node"
                            );
                            return Ok(Some(node_result));
                        }
                        trace!(
                            %node_result,
                            "System node does not match requested version"
                        );
                    }
                    Err(e) => {
                        warn!(?e, "Failed to get version for system node");
                    }
                }
            } else {
                trace!(
                    node = %node_path.display(),
                    "No npm found in same directory as node executable"
                );
            }
        }

        debug!(
            ?node_request,
            "No system node matches the requested version"
        );
        Ok(None)
    }

    /// Find npm executable in the same directory as the given node executable.
    fn find_npm_in_same_directory(node_path: &Path) -> Result<Option<PathBuf>> {
        let node_dir = node_path
            .parent()
            .context("Node executable has no parent directory")?;

        for name in ["npm", "npm.cmd", "npm.bat"] {
            let npm_path = node_dir.join(name);
            if npm_path.try_exists()? && is_executable(&npm_path) {
                trace!(
                    node = %node_path.display(),
                    npm = %npm_path.display(),
                    "Found npm in same directory as node"
                );
                return Ok(Some(npm_path));
            }
        }
        trace!(
            node = %node_path.display(),
            "npm not found in same directory as node"
        );
        Ok(None)
    }
}

pub(crate) fn bin_dir(prefix: &Path) -> PathBuf {
    if cfg!(windows) {
        prefix.to_path_buf()
    } else {
        prefix.join("bin")
    }
}

pub(crate) fn lib_dir(prefix: &Path) -> PathBuf {
    if cfg!(windows) {
        prefix.join("node_modules")
    } else {
        prefix.join("lib").join("node_modules")
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(windows)]
    {
        path.extension()
            .is_some_and(|ext| ext == EXE_EXTENSION || ext == "cmd" || ext == "bat")
    }
    #[cfg(not(windows))]
    {
        use std::os::unix::fs::MetadataExt;
        path.is_file() && fs_err::metadata(path).is_ok_and(|m| m.mode() & 0o111 != 0)
    }
}
