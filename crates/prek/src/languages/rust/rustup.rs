use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use target_lexicon::HOST;
use tracing::{debug, trace};

use crate::checksum::{Sha256Digest, fetch_checksum};
use crate::fs::{LockedFile, make_executable};
use crate::http::download_artifact;
use crate::process::Cmd;
use crate::store::Store;
use crate::warn_user;

/// A rustup executable for inspecting and installing toolchains.
pub(crate) struct Rustup {
    bin: PathBuf,
}

static RUSTUP_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    EnvVars
        .var(EnvVars::PREK_INTERNAL__RUSTUP_BINARY_NAME)
        .unwrap_or_else(|_| "rustup".to_string())
});

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, strum::AsRefStr, strum::EnumString)]
#[strum(serialize_all = "lowercase")]
enum RustupProfile {
    #[default]
    Minimal,
    Default,
    Complete,
}

fn rustup_profile(env_vars: &impl EnvVarsRead) -> RustupProfile {
    let Ok(value) = env_vars.var(EnvVars::PREK_RUST_PROFILE) else {
        return RustupProfile::default();
    };

    value.parse().unwrap_or_else(|_| {
        let default = RustupProfile::default();
        warn_user!(
            "Invalid value for {}: {:?}. Expected minimal, default, or complete; using default ({:?})",
            EnvVars::PREK_RUST_PROFILE,
            value,
            default.as_ref(),
        );

        default
    })
}

impl Rustup {
    /// Find prek's cached rustup, falling back to PATH.
    pub(crate) fn find(managed_root: &Path) -> Option<Self> {
        let bin = managed_root.join("rustup").with_extension(EXE_EXTENSION);
        if bin.is_file() {
            return Some(Self { bin });
        }

        let bin = which::which(&*RUSTUP_BINARY_NAME).ok()?;

        Some(Self { bin })
    }

    /// Install rustup if not already installed.
    pub(crate) async fn install(store: &Store, rustup_home: &Path) -> Result<Self> {
        if let Some(rustup) = Self::find(rustup_home) {
            return Ok(rustup);
        }

        fs_err::tokio::create_dir_all(&rustup_home).await?;
        let _lock = LockedFile::acquire(rustup_home.join(".lock"), "rustup").await?;

        if let Some(rustup) = Self::find(rustup_home) {
            trace!("Using rustup at {}", rustup.bin.display());
            return Ok(rustup);
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
        let checksum_url = format!("{url}.sha256");

        let download = download_artifact(&url, filename, store, async || {
            let Some(checksum) = fetch_checksum(&checksum_url).await? else {
                return Ok(None);
            };

            digest_from_rustup_checksum(&checksum)
        })
        .await?;
        make_executable(download.path())?;

        debug!(path = %target.display(), "Installing rustup");
        crate::fs::rename_with_retry(download.path(), &target).await?;

        Ok(Self { bin: target })
    }

    /// Install a toolchain into the given rustup home and return its directory.
    pub(crate) async fn install_toolchain(
        &self,
        rustup_home: &Path,
        toolchain: &str,
    ) -> Result<PathBuf> {
        let output = Cmd::new(&self.bin)
            .env(EnvVars::RUSTUP_HOME, rustup_home)
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
            .arg("toolchain")
            .arg("install")
            .arg("--no-self-update")
            .arg("--profile")
            .arg(rustup_profile(&EnvVars).as_ref())
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
                let (name, _) = line.trim().split_once(" installed")?;
                Some(name.trim())
            })
            .with_context(|| {
                format!(
                    "Unable to detect installed toolchain name from rustup output for `{toolchain}`"
                )
            })?;

        Ok(rustup_home.join("toolchains").join(installed_name))
    }

    /// List toolchain directories in the current rustup home.
    pub(crate) async fn list_toolchains(&self) -> Result<Vec<PathBuf>> {
        let output = Cmd::new(&self.bin)
            .arg("toolchain")
            .arg("list")
            .arg("-v")
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
            .check(true)
            .output()
            .await
            .context("Failed to list Rust toolchains")?;

        let entries = str::from_utf8(&output.stdout)?
            .lines()
            .filter_map(parse_toolchain_line)
            .collect();

        Ok(entries)
    }

    pub(crate) async fn resolve_rustc(executable: &Path, cwd: &Path) -> Result<PathBuf> {
        let resolved = dunce::canonicalize(executable)?;
        let rustup = resolved
            .with_file_name("rustup")
            .with_extension(EXE_EXTENSION);

        // rustup proxies can be hard links, so canonicalizing alone does not identify them.
        if !same_file::is_same_file(&resolved, &rustup).unwrap_or(false) {
            return Ok(executable.to_path_buf());
        }

        let output = Cmd::new(&rustup)
            .args(["which", "rustc"])
            .current_dir(cwd)
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
            .check(true)
            .output()
            .await?;

        Ok(PathBuf::from(str::from_utf8(&output.stdout)?.trim()))
    }
}

fn digest_from_rustup_checksum(contents: &str) -> Result<Option<Sha256Digest>> {
    let Some(digest) = contents.split_whitespace().next() else {
        return Ok(None);
    };

    digest.parse().map(Some)
}

fn parse_toolchain_line(line: &str) -> Option<PathBuf> {
    // Typical formats:
    // "stable-aarch64-apple-darwin (default) /Users/me/.rustup/toolchains/stable-aarch64-apple-darwin"
    // "nightly-x86_64-unknown-linux-gnu /home/me/.rustup/toolchains/nightly-x86_64-unknown-linux-gnu"
    line.split_whitespace()
        .next_back()
        .map(PathBuf::from)
        .filter(|path| path.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    const EMPTY_SHA256: &str = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";

    #[test]
    fn parses_rustup_checksum() -> Result<()> {
        let digest = digest_from_rustup_checksum(EMPTY_SHA256)?;
        assert_eq!(digest.unwrap().to_string(), EMPTY_SHA256);

        let digest = digest_from_rustup_checksum(&format!("{EMPTY_SHA256}  rustup-init\n"))?;
        assert_eq!(digest.unwrap().to_string(), EMPTY_SHA256);

        let result = digest_from_rustup_checksum("")?;
        assert!(result.is_none());
        Ok(())
    }

    #[test]
    fn rustup_profile_reads_env() {
        fn profile_with(value: &str) -> RustupProfile {
            rustup_profile(&EnvVars::from_map(&[(EnvVars::PREK_RUST_PROFILE, value)]))
        }

        assert_eq!(
            rustup_profile(&EnvVars::from_map(&[])),
            RustupProfile::Minimal
        );
        assert_eq!(profile_with("default"), RustupProfile::Default);
        assert_eq!(profile_with("complete"), RustupProfile::Complete);
        assert_eq!(profile_with("invalid"), RustupProfile::Minimal);
    }
}
