use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use crate::fs::LockedFile;
use crate::languages::REQWEST_CLIENT;
use crate::languages::rust::version::RustVersion;
use crate::process::Cmd;
use crate::store::Store;

use anyhow::{Context, Result};
use futures::{StreamExt, TryStreamExt, stream};
use prek_consts::env_vars::EnvVars;
use semver::Version;
use target_lexicon::HOST;
use tracing::{debug, trace};

#[derive(Clone)]
pub(crate) struct Rustup {
    bin: PathBuf,
    rustup_home: PathBuf,
}

pub(crate) struct ToolchainInfo {
    pub(crate) name: String,
    pub(crate) path: PathBuf,
    pub(crate) version: RustVersion,
}

static RUSTUP_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    EnvVars::var(EnvVars::PREK_INTERNAL__RUSTUP_BINARY_NAME)
        .unwrap_or_else(|_| "rustup".to_string())
});

impl Rustup {
    pub(crate) fn rustup_home(&self) -> &Path {
        &self.rustup_home
    }

    /// Install rustup if not already installed.
    pub(crate) async fn install(store: &Store, rustup_home: &Path) -> Result<Self> {
        // 1) Check system installed `rustup`
        if let Ok(rustup_path) = which::which(&*RUSTUP_BINARY_NAME) {
            trace!("Using system installed rustup at {}", rustup_path.display());
            return Ok(Self {
                bin: rustup_path,
                rustup_home: rustup_home.to_path_buf(),
            });
        }

        // 2) Check if already installed in store
        let rustup_path = rustup_home.join("rustup").with_extension(EXE_EXTENSION);

        if rustup_path.is_file() {
            trace!("Using managed rustup at {}", rustup_path.display());
            return Ok(Self {
                bin: rustup_path,
                rustup_home: rustup_home.to_path_buf(),
            });
        }

        // 3) Install rustup
        fs_err::tokio::create_dir_all(&rustup_home).await?;
        let _lock = LockedFile::acquire(rustup_home.join(".lock"), "rustup").await?;

        if rustup_path.is_file() {
            trace!("Using managed rustup at {}", rustup_path.display());
            return Ok(Self {
                bin: rustup_path,
                rustup_home: rustup_home.to_path_buf(),
            });
        }

        Self::download(store, rustup_home)
            .await
            .context("Failed to install rustup")
    }

    async fn download(store: &Store, rustup_home: &Path) -> Result<Self> {
        let triple = HOST.to_string();
        let filename = if cfg!(windows) {
            "rustup-init.exe"
        } else {
            "rustup-init"
        };
        let url = format!("https://static.rust-lang.org/rustup/dist/{triple}/{filename}");
        // Save "rustup-init" as "rustup", this is what "rustup-init" does when setting up.
        let target = rustup_home.join("rustup").with_extension(EXE_EXTENSION);

        let temp_dir = tempfile::tempdir_in(store.scratch_path())?;
        debug!(url = %url, temp_dir = ?temp_dir.path(), "Downloading");

        let tmp_target = temp_dir.path().join(filename);
        let response = REQWEST_CLIENT
            .get(&url)
            .send()
            .await
            .with_context(|| format!("Failed to download file from {url}"))?;
        if !response.status().is_success() {
            anyhow::bail!(
                "Failed to download file from {}: {}",
                url,
                response.status()
            );
        }

        let bytes = response.bytes().await?;
        fs_err::tokio::write(&tmp_target, bytes).await?;

        make_executable(&tmp_target)?;

        // Move to final location
        if target.exists() {
            debug!(path = %target.display(), "Removing existing rustup");
            fs_err::tokio::remove_file(&target).await?;
        }
        debug!(path = %target.display(), "Installing rustup");
        fs_err::tokio::rename(&tmp_target, &target).await?;

        Ok(Self {
            bin: target,
            rustup_home: rustup_home.to_path_buf(),
        })
    }

    pub(crate) async fn install_toolchain(&self, toolchain: &str) -> Result<PathBuf> {
        let output = Cmd::new(&self.bin, "rustup toolchain install")
            .env(EnvVars::RUSTUP_HOME, &self.rustup_home)
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
            .arg("toolchain")
            .arg("install")
            .arg("--no-self-update")
            .arg("--profile")
            .arg("minimal")
            .arg(toolchain)
            .check(true)
            .output()
            .await
            .with_context(|| format!("Failed to install rust toolchain {toolchain}"))?;

        // Parse installed toolchain name from output
        let stdout = String::from_utf8_lossy(&output.stdout);
        let installed_name = stdout
            .lines()
            .find_map(|line| {
                let line = line.trim();
                let (name, _) = line.split_once(" installed")?;
                let name = name.trim();
                if name.is_empty() {
                    None
                } else {
                    Some(name.to_string())
                }
            })
            .with_context(|| {
                format!(
                    "Unable to detect installed toolchain name from rustup output for `{toolchain}`"
                )
            })?;

        Ok(self.rustup_home.join("toolchains").join(installed_name))
    }

    /// List installed toolchains managed by prek.
    pub(crate) async fn list_installed_toolchains(&self) -> Result<Vec<ToolchainInfo>> {
        let output = Cmd::new(&self.bin, "rustup list toolchains")
            .arg("toolchain")
            .arg("list")
            .arg("-v")
            .env(EnvVars::RUSTUP_HOME, &self.rustup_home)
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
            .check(true)
            .output()
            .await
            .context("Failed to list installed toolchains")?;

        let entries: Vec<(String, PathBuf)> = str::from_utf8(&output.stdout)?
            .lines()
            .filter_map(parse_toolchain_line)
            .collect();

        let infos: Vec<ToolchainInfo> = stream::iter(entries)
            .map(async move |(name, path)| toolchain_info(name, path).await)
            .buffer_unordered(8)
            .try_collect()
            .await?;

        Ok(infos)
    }

    /// List system-installed Rust toolchains.
    pub(crate) async fn list_system_toolchains(&self) -> Result<Vec<ToolchainInfo>> {
        let output = Cmd::new(&self.bin, "rustup toolchain list")
            .arg("toolchain")
            .arg("list")
            .arg("-v")
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
            .check(true)
            .output()
            .await
            .context("Failed to list system toolchains")?;

        let entries: Vec<(String, PathBuf)> = str::from_utf8(&output.stdout)?
            .lines()
            .filter_map(parse_toolchain_line)
            .collect();

        let infos: Vec<ToolchainInfo> = stream::iter(entries)
            .map(async move |(name, path)| toolchain_info(name, path).await)
            .buffer_unordered(8)
            .try_collect()
            .await?;

        Ok(infos)
    }
}

fn parse_toolchain_line(line: &str) -> Option<(String, PathBuf)> {
    // Typical formats:
    // "stable-aarch64-apple-darwin (default) /Users/me/.rustup/toolchains/stable-aarch64-apple-darwin"
    // "nightly-x86_64-unknown-linux-gnu /home/me/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu"
    let parts: Vec<_> = line.split_whitespace().collect();
    let name = (*parts.first()?).to_string();
    let path = parts.last()?;
    let path = PathBuf::from(path);
    if path.exists() {
        Some((name, path))
    } else {
        None
    }
}

async fn toolchain_info(name: String, toolchain_dir: PathBuf) -> Result<ToolchainInfo> {
    let rustc = toolchain_dir
        .join("bin")
        .join("rustc")
        .with_extension(EXE_EXTENSION);

    let output = Cmd::new(&rustc, "rustc version")
        .arg("--version")
        .check(true)
        .output()
        .await
        .with_context(|| format!("Failed to read version from {}", rustc.display()))?;

    let version_str = str::from_utf8(&output.stdout)?
        .split_whitespace()
        .nth(1)
        .context("Failed to parse rustc --version output")?;
    let version = Version::parse(version_str)?;
    let version = RustVersion::from_path(&version, &toolchain_dir);

    Ok(ToolchainInfo {
        name,
        path: toolchain_dir,
        version,
    })
}

fn make_executable(path: &Path) -> std::io::Result<()> {
    #[allow(clippy::unnecessary_wraps)]
    #[cfg(windows)]
    fn inner(_: &Path) -> std::io::Result<()> {
        Ok(())
    }
    #[cfg(not(windows))]
    fn inner(path: &Path) -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;

        let metadata = fs_err::metadata(path)?;
        let mut perms = metadata.permissions();
        let mode = perms.mode();
        let new_mode = (mode & !0o777) | 0o755;

        // Check if permissions are ok already
        if mode == new_mode {
            return Ok(());
        }

        perms.set_mode(new_mode);
        fs_err::set_permissions(path, perms)
    }

    inner(path)
}
