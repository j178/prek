use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::languages::rust::RustRequest;
use crate::languages::rust::version::RustVersion;
use crate::process::Cmd;
use crate::store::Store;

pub(crate) struct RustResult {
    path: PathBuf,
    version: RustVersion,
    from_system: bool,
}

impl Display for RustResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.path.display(), self.version)?;
        Ok(())
    }
}

/// Override the Rust binary name for testing.
static RUST_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars::var(EnvVars::PREK_INTERNAL__RUST_BINARY_NAME) {
        name
    } else {
        "rustc".to_string()
    }
});

impl RustResult {
    fn from_executable(path: PathBuf, from_system: bool) -> Self {
        Self {
            path,
            from_system,
            version: RustVersion::default(),
        }
    }

    pub(crate) fn from_dir(dir: &Path, from_system: bool) -> Self {
        let rustc = bin_dir(dir).join("rustc").with_extension(EXE_EXTENSION);
        Self::from_executable(rustc, from_system)
    }

    pub(crate) fn bin(&self) -> &Path {
        &self.path
    }

    pub(crate) fn version(&self) -> &RustVersion {
        &self.version
    }

    pub(crate) fn is_from_system(&self) -> bool {
        self.from_system
    }

    pub(crate) fn cmd(&self, summary: &str) -> Cmd {
        let mut cmd = Cmd::new(&self.path, summary);
        cmd.env(EnvVars::RUSTUP_AUTO_INSTALL, "0");
        cmd
    }

    pub(crate) fn with_version(mut self, version: RustVersion) -> Self {
        self.version = version;
        self
    }

    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        let output = self
            .cmd("rustc version")
            .arg("--version")
            .check(true)
            .output()
            .await?;

        // e.g. "rustc 1.70.0 (90c541806 2023-05-31)"
        let version_str = str::from_utf8(&output.stdout)?;
        let version_str = version_str.split_ascii_whitespace().nth(1).ok_or_else(|| {
            anyhow::anyhow!("Failed to parse Rust version from output: {version_str}")
        })?;

        let version = RustVersion::from_str(version_str)?;

        self.version = version;

        Ok(self)
    }

    pub(crate) async fn fill_version_with_toolchain(
        mut self,
        toolchain: &str,
        rustup_home: &Path,
    ) -> Result<Self> {
        let output = self
            .cmd("rustc version")
            .arg("--version")
            .env(EnvVars::RUSTUP_TOOLCHAIN, toolchain)
            .env(EnvVars::RUSTUP_HOME, rustup_home)
            .check(true)
            .output()
            .await?;

        // e.g. "rustc 1.70.0 (90c541806 2023-05-31)"
        let version_str = str::from_utf8(&output.stdout)?;
        let version_str = version_str.split_ascii_whitespace().nth(1).ok_or_else(|| {
            anyhow::anyhow!("Failed to parse Rust version from output: {version_str}")
        })?;

        let version = RustVersion::from_str(version_str)?;
        self.version = version;

        Ok(self)
    }
}

pub(crate) struct RustInstaller {
    root: PathBuf,
}

impl RustInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub(crate) async fn install(
        &self,
        _store: &Store,
        request: &RustRequest,
        allows_download: bool,
    ) -> Result<RustResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;
        let _lock = LockedFile::acquire(self.root.join(".lock"), "rust").await?;

        // Check cache first
        if let Ok(rust) = self.find_installed(request) {
            trace!(%rust, "Found installed rust");
            let toolchain = rust
                .bin()
                .parent()
                .and_then(|p| p.parent())
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .map(ToString::to_string)
                .context("Failed to extract toolchain name")?;
            let rustup_home = rustup_home_dir(
                rust.bin()
                    .parent()
                    .and_then(|p| p.parent())
                    .context("Failed to get rust dir")?,
            );
            return rust
                .fill_version_with_toolchain(&toolchain, &rustup_home)
                .await;
        }

        // Check system second
        if let Some(rust) = self.find_system_rust(request).await? {
            trace!(%rust, "Using system rust");
            return Ok(rust);
        }

        if !allows_download {
            anyhow::bail!("No suitable system Rust version found and downloads are disabled");
        }

        let toolchain = self.resolve_version(request).await?;
        let target_dir = self.root.join(&toolchain);
        install_rust_with_toolchain(&toolchain, &target_dir).await?;

        let rustup_home = rustup_home_dir(&target_dir);
        RustResult::from_dir(&target_dir, false)
            .fill_version_with_toolchain(&toolchain, &rustup_home)
            .await
    }

    fn find_installed(&self, request: &RustRequest) -> Result<RustResult> {
        let mut installed = fs_err::read_dir(&self.root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(e) => {
                    warn!(?e, "Failed to read entry");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_ok_and(|f| f.is_dir()))
            .map(|entry| {
                let dir_name = entry.file_name();
                let dir_str = dir_name.to_string_lossy();

                // Try to parse as version, or use as channel name
                let version = RustVersion::from_str(&dir_str).ok();
                (dir_str.to_string(), version, entry.path())
            })
            .sorted_unstable_by(|(_, a, _), (_, b, _)| {
                // Sort by version if available, otherwise by name
                match (a, b) {
                    (Some(a), Some(b)) => b.cmp(a), // reverse for newest first
                    _ => std::cmp::Ordering::Equal,
                }
            });

        installed
            .find_map(|(name, version, path)| {
                let matches = match (request, version) {
                    (RustRequest::Channel(ch), _) => &name == ch,
                    (_, Some(v)) => request.matches(&v, Some(&path)),
                    _ => false,
                };

                if matches {
                    trace!(name = %name, "Found matching installed rust");
                    Some(RustResult::from_dir(&path, false))
                } else {
                    trace!(name = %name, "Installed rust does not match request");
                    None
                }
            })
            .context("No installed rust version matches the request")
    }

    async fn find_system_rust(&self, rust_request: &RustRequest) -> Result<Option<RustResult>> {
        let rust_paths = match which::which_all(&*RUST_BINARY_NAME) {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No rustc executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        for rust_path in rust_paths {
            match RustResult::from_executable(rust_path, true)
                .fill_version()
                .await
            {
                Ok(rust) => {
                    // Check if this version matches the request
                    if rust_request.matches(&rust.version, Some(&rust.path)) {
                        trace!(
                            %rust,
                            "Found matching system rust"
                        );
                        return Ok(Some(rust));
                    }
                    trace!(
                        %rust,
                        "System rust does not match requested version"
                    );
                }
                Err(e) => {
                    warn!(?e, "Failed to get version for system rust");
                }
            }
        }

        debug!(
            ?rust_request,
            "No system rust matches the requested version"
        );
        Ok(None)
    }

    async fn resolve_version(&self, req: &RustRequest) -> Result<String> {
        match req {
            RustRequest::Any => Ok("stable".to_string()),
            RustRequest::Channel(ch) => Ok(ch.clone()),
            RustRequest::Path(p) => Ok(p.to_string_lossy().to_string()),

            RustRequest::Major(_)
            | RustRequest::MajorMinor(_, _)
            | RustRequest::MajorMinorPatch(_, _, _)
            | RustRequest::Range(_, _) => {
                let output = crate::git::git_cmd("list rust tags")?
                    .arg("ls-remote")
                    .arg("--tags")
                    .arg("https://github.com/rust-lang/rust")
                    .output()
                    .await?
                    .stdout;
                let versions: Vec<RustVersion> = str::from_utf8(&output)?
                    .lines()
                    .filter_map(|line| {
                        let tag = line.split('\t').nth(1)?;
                        let tag = tag.strip_prefix("refs/tags/")?;
                        RustVersion::from_str(tag).ok()
                    })
                    .sorted_unstable_by(|a, b| b.cmp(a))
                    .collect();

                let version = versions
                    .into_iter()
                    .find(|version| req.matches(version, None))
                    .context("Version not found on remote")?;
                Ok(version.to_string())
            }
        }
    }
}

pub(crate) fn bin_dir(env_path: &Path) -> PathBuf {
    env_path.join("bin")
}

/// Returns the path to the `RUSTUP_HOME` directory within a managed rust installation.
pub(crate) fn rustup_home_dir(env_path: &Path) -> PathBuf {
    env_path.join("rustup")
}

#[cfg(unix)]
fn make_executable(filename: impl AsRef<Path>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = std::fs::metadata(filename.as_ref())?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    std::fs::set_permissions(filename.as_ref(), permissions)?;

    Ok(())
}

#[cfg(not(unix))]
#[allow(clippy::unnecessary_wraps)]
fn make_executable(_filename: impl AsRef<Path>) -> std::io::Result<()> {
    Ok(())
}

async fn install_rust_with_toolchain(toolchain: &str, target_dir: &Path) -> Result<()> {
    // Use a persistent RUSTUP_HOME within the target directory so toolchains are preserved
    let rustup_home = rustup_home_dir(target_dir);
    fs_err::tokio::create_dir_all(&rustup_home).await?;

    let rustup_bin = bin_dir(target_dir)
        .join("rustup")
        .with_extension(EXE_EXTENSION);

    // Check if rustup already exists at the expected location
    if !rustup_bin.exists() {
        // Download rustup-init to a temporary location
        let rustup_init_dir = tempfile::tempdir()?;

        let url = if cfg!(windows) {
            "https://win.rustup.rs/x86_64"
        } else {
            "https://sh.rustup.rs"
        };

        let resp = crate::languages::REQWEST_CLIENT
            .get(url)
            .send()
            .await
            .context("Failed to download rustup-init")?;

        if !resp.status().is_success() {
            anyhow::bail!("Failed to download rustup-init: {}", resp.status());
        }

        let rustup_init = rustup_init_dir
            .path()
            .join("rustup-init")
            .with_extension(EXE_EXTENSION);
        fs_err::tokio::write(&rustup_init, resp.bytes().await?).await?;
        make_executable(&rustup_init)?;

        // Install rustup into CARGO_HOME/bin, with RUSTUP_HOME in the persistent location
        Cmd::new(&rustup_init, "install rustup")
            .args([
                "-y",
                "--quiet",
                "--no-modify-path",
                "--default-toolchain",
                "none",
            ])
            .env(EnvVars::CARGO_HOME, target_dir)
            .env(EnvVars::RUSTUP_HOME, &rustup_home)
            .check(true)
            .output()
            .await?;
    }

    // Install the requested toolchain into our persistent RUSTUP_HOME
    Cmd::new(&rustup_bin, "install toolchain")
        .args(["toolchain", "install", "--no-self-update", toolchain])
        .env(EnvVars::CARGO_HOME, target_dir)
        .env(EnvVars::RUSTUP_HOME, &rustup_home)
        .check(true)
        .output()
        .await?;

    Ok(())
}
