use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::string::ToString;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use asyncband::once::OnceMap;
use itertools::Itertools;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use rustc_hash::FxBuildHasher;
use serde::Deserialize;
use target_lexicon::{Architecture, HOST, OperatingSystem, Triple};
use tracing::{debug, trace, warn};

use crate::archive;
use crate::checksum::{digest_from_sha256sums, fetch_checksum};
use crate::fs::{LockedFile, is_executable};
use crate::http::{download_artifact, reqwest_client};
use crate::languages::node::NodeRequest;
use crate::languages::node::version::NodeVersion;
use crate::languages::version::{ToolchainPolicy, ToolchainSource, find_system_executables};
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug)]
pub(crate) struct NodeResult {
    node: PathBuf,
    npm: PathBuf,
    version: Arc<NodeVersion>,
}

impl Display for NodeResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.node.display(), self.version)?;
        Ok(())
    }
}

/// Override the Node binary name for testing.
static NODE_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars.var(EnvVars::PREK_INTERNAL__NODE_BINARY_NAME) {
        name
    } else {
        "node".to_string()
    }
});

impl NodeResult {
    pub(crate) fn from_dir(dir: &Path, version: NodeVersion) -> Self {
        let node = bin_dir(dir).join("node").with_extension(EXE_EXTENSION);
        let npm = bin_dir(dir)
            .join("npm")
            .with_extension(if cfg!(windows) { "cmd" } else { "" });
        Self {
            node,
            npm,
            version: Arc::new(version),
        }
    }

    pub(crate) async fn from_executables(node: PathBuf, npm: PathBuf) -> Result<Self> {
        let version = query_node_version_cached(&node).await?;
        Ok(Self { node, npm, version })
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

// Canonical paths let hook environments backed by the same Node executable share one query.
static NODE_VERSION_CACHE: LazyLock<OnceMap<PathBuf, Arc<NodeVersion>, FxBuildHasher>> =
    LazyLock::new(|| OnceMap::with_hasher(FxBuildHasher));

async fn query_node_version(node: &Path) -> Result<NodeVersion> {
    // https://nodejs.org/api/process.html#processrelease
    let output = Cmd::new(node)
        .arg("-p")
        .arg("JSON.stringify({version: process.version, lts: process.release.lts || false})")
        .check(true)
        .output()
        .await?;
    serde_json::from_slice(&output.stdout).context("Failed to parse node version")
}

pub(crate) async fn query_node_version_cached(node: &Path) -> Result<Arc<NodeVersion>> {
    let node = fs_err::canonicalize(node).unwrap_or_else(|_| node.to_path_buf());
    NODE_VERSION_CACHE
        .try_compute(node.clone(), async move || {
            query_node_version(&node).await.map(Arc::new)
        })
        .await
}

pub(crate) struct NodeInstaller {
    root: PathBuf,
}

#[derive(Deserialize)]
struct NodeRelease {
    #[serde(flatten)]
    version: NodeVersion,
    files: Vec<String>,
}

impl NodeRelease {
    fn archive_filename(&self, host: &Triple) -> Result<Option<String>> {
        let mut arch = match host.architecture {
            Architecture::X86_32(_) => "x86",
            Architecture::X86_64 => "x64",
            Architecture::Aarch64(_) => "arm64",
            Architecture::Arm(_) => "armv7l",
            Architecture::S390x => "s390x",
            Architecture::Powerpc => "ppc64",
            Architecture::Powerpc64le => "ppc64le",
            _ => anyhow::bail!("Unsupported architecture"),
        };
        let os = match host.operating_system {
            OperatingSystem::Darwin(_) => "darwin",
            OperatingSystem::Linux => "linux",
            OperatingSystem::Windows => "win",
            OperatingSystem::Aix => "aix",
            _ => anyhow::bail!("Unsupported OS"),
        };
        if os == "darwin" && arch == "arm64" && self.version.major() < 16 {
            // Node.js 16 and later are required for arm64 on macOS.
            arch = "x64";
        }

        let file = match os {
            "darwin" => format!("osx-{arch}-tar"),
            "win" => format!("win-{arch}-zip"),
            _ => format!("{os}-{arch}"),
        };
        if !self.files.contains(&file) {
            return Ok(None);
        }
        let ext = if os == "win" { "zip" } else { "tar.gz" };

        Ok(Some(format!(
            "node-v{}-{os}-{arch}.{ext}",
            self.version.version()
        )))
    }
}

impl NodeInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Install a version of Node.js.
    pub(crate) async fn install(
        &self,
        store: &Store,
        request: &NodeRequest,
        policy: ToolchainPolicy,
    ) -> Result<NodeResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;

        let _lock = LockedFile::acquire(self.root.join(".lock"), "node").await?;

        for &source in policy.search_order() {
            let result = match source {
                ToolchainSource::Managed => self.find_installed(request).ok(),
                ToolchainSource::System => self.find_system_node(request).await?,
            };
            if let Some(result) = result {
                trace!(%result, ?source, "Found node");
                return Ok(result);
            }
        }

        if !policy.allows_download() {
            anyhow::bail!("No suitable Node version found for toolchain policy: {policy}");
        }

        let releases = self
            .list_remote_versions()
            .await
            .context("Failed to list remote versions")?;
        let (resolved_version, filename) = Self::resolve_version(request, releases, &HOST)?;
        trace!(version = %resolved_version, "Downloading node");

        self.download(store, &resolved_version, &filename).await
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
                if req.matches(&v) {
                    Some(NodeResult::from_dir(&path, v))
                } else {
                    None
                }
            })
            .context("No installed node version matches the request")
    }

    fn resolve_version(
        req: &NodeRequest,
        releases: Vec<NodeRelease>,
        host: &Triple,
    ) -> Result<(NodeVersion, String)> {
        // Latest versions come first, so we can find the latest matching version.
        for release in releases {
            let version = &release.version;
            // Only Range requests can name a prerelease, e.g. `=27.0.0-alpha.0.0.0`.
            // Let `req.matches` check those; reject prereleases for other requests.
            if !version.version.pre.is_empty() && !matches!(req, NodeRequest::Range(_)) {
                continue;
            }
            if !req.matches(version) {
                continue;
            }
            if let Some(filename) = release.archive_filename(host)? {
                return Ok((release.version, filename));
            }
        }
        anyhow::bail!("No matching Node.js version with an archive for {host}")
    }

    /// List all versions of Node.js available on the Node.js website.
    async fn list_remote_versions(&self) -> Result<Vec<NodeRelease>> {
        let url = "https://nodejs.org/dist/index.json";
        let versions = reqwest_client()?.get(url).send().await?.json().await?;
        Ok(versions)
    }

    // TODO: support mirror?
    /// Install a specific version of Node.js.
    async fn download(
        &self,
        store: &Store,
        version: &NodeVersion,
        filename: &str,
    ) -> Result<NodeResult> {
        let url = format!("https://nodejs.org/dist/v{}/{filename}", version.version());
        let checksum_url = format!(
            "https://nodejs.org/dist/v{}/SHASUMS256.txt",
            version.version()
        );
        let target = self.root.join(version.to_string());

        let download = download_artifact(&url, filename, store, async || {
            let Some(checksums) = fetch_checksum(&checksum_url).await? else {
                return Ok(None);
            };
            digest_from_sha256sums(&checksums, filename)
        })
        .await
        .context("Failed to download node")?;
        let extracted = archive::extract_archive(download.path())
            .await
            .context("Failed to extract node")?;
        if target.exists() {
            debug!(target = %target.display(), "Removing existing node");
            fs_err::tokio::remove_dir_all(&target).await?;
        }

        debug!(?extracted, target = %target.display(), "Moving node to target");
        crate::fs::rename_with_retry(&extracted, &target).await?;

        Ok(NodeResult::from_dir(&target, version.clone()))
    }

    /// Find a suitable system Node.js installation that matches the request.
    async fn find_system_node(&self, node_request: &NodeRequest) -> Result<Option<NodeResult>> {
        let node_paths = match find_system_executables(&*NODE_BINARY_NAME, &self.root) {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No node executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        // Check each node executable for a matching version, stop early if found
        for node_path in node_paths {
            if let Some(npm_path) = Self::find_npm_in_same_directory(&node_path)? {
                match NodeResult::from_executables(node_path, npm_path).await {
                    Ok(node_result) => {
                        // Check if this version matches the request
                        if node_request.matches(&node_result.version) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// An alpha at the head of the remote index must not become the default
    /// download, but an explicit prerelease request must still select it.
    #[test]
    fn resolve_version_requires_explicit_prerelease() -> Result<()> {
        let index = r#"[
            {"version":"v27.0.0-alpha.0.0.0","lts":false,"files":["linux-x64"]},
            {"version":"v26.13.0","lts":"Lithium","files":["linux-x64"]}
        ]"#;
        let host = "x86_64-unknown-linux-gnu"
            .parse()
            .map_err(anyhow::Error::msg)?;

        for (request, expected) in [
            ("", Some("26.13.0")),
            ("27", None),
            ("27.0", None),
            ("27.0.0", None),
            (">=26", Some("26.13.0")),
            ("=27.0.0-alpha.0.0.0", Some("27.0.0-alpha.0.0.0")),
        ] {
            let result = NodeInstaller::resolve_version(
                &request.parse()?,
                serde_json::from_str(index)?,
                &host,
            );
            if let Some(expected) = expected {
                assert_eq!(result?.0.version().to_string(), expected, "{request:?}");
            } else {
                assert!(result.is_err(), "{request:?}");
            }
        }

        Ok(())
    }

    /// Select the newest matching release with a usable archive. A pkg or msi
    /// must not count as a tar or zip, and fallback must respect the request.
    #[test]
    fn resolve_version_skips_unavailable_archives() -> Result<()> {
        let index = r#"[
            {
                "version": "v27.0.0",
                "lts": false,
                "files": ["osx-arm64-tar", "osx-x64-pkg", "win-x64-msi", "linux-x64"]
            },
            {
                "version": "v26.13.0",
                "lts": "Lithium",
                "files": ["osx-x64-tar", "win-x64-zip"]
            }
        ]"#;

        for (host, request, expected) in [
            (
                "x86_64-apple-darwin",
                "",
                Some("node-v26.13.0-darwin-x64.tar.gz"),
            ),
            (
                "x86_64-apple-darwin",
                ">=26",
                Some("node-v26.13.0-darwin-x64.tar.gz"),
            ),
            (
                "x86_64-apple-darwin",
                "lts",
                Some("node-v26.13.0-darwin-x64.tar.gz"),
            ),
            ("x86_64-apple-darwin", "27", None),
            ("x86_64-apple-darwin", "27.0.0", None),
            (
                "x86_64-pc-windows-msvc",
                "",
                Some("node-v26.13.0-win-x64.zip"),
            ),
            (
                "aarch64-apple-darwin",
                "",
                Some("node-v27.0.0-darwin-arm64.tar.gz"),
            ),
            (
                "x86_64-unknown-linux-gnu",
                "",
                Some("node-v27.0.0-linux-x64.tar.gz"),
            ),
        ] {
            let host = host.parse().map_err(anyhow::Error::msg)?;
            let result = NodeInstaller::resolve_version(
                &request.parse()?,
                serde_json::from_str(index)?,
                &host,
            );
            if let Some(expected) = expected {
                assert_eq!(result?.1, expected, "{host}: {request:?}");
            } else {
                assert!(result.is_err(), "{host}: {request:?}");
            }
        }

        Ok(())
    }

    /// Apple Silicon must still be able to download an old Node version using
    /// its x64 archive, even when no native arm64 archive is listed.
    #[test]
    fn resolve_old_node_on_macos_arm64() -> Result<()> {
        let index = r#"[
            {"version":"v14.21.3","lts":"Fermium","files":["osx-x64-tar"]}
        ]"#;
        let host = "aarch64-apple-darwin".parse().map_err(anyhow::Error::msg)?;
        let (_, filename) =
            NodeInstaller::resolve_version(&"14".parse()?, serde_json::from_str(index)?, &host)?;
        assert_eq!(filename, "node-v14.21.3-darwin-x64.tar.gz");
        Ok(())
    }
}
