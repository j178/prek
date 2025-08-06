use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::TryStreamExt;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::trace;

use crate::archive::ArchiveExtension;
use crate::config::Language;
use crate::hook::{Hook, InstalledHook};
use crate::store::Store;
use crate::{archive, builtin};

mod docker;
mod docker_image;
mod fail;
mod golang;
mod node;
mod python;
mod script;
mod system;
pub mod version;

static GOLANG: golang::Golang = golang::Golang;
static PYTHON: python::Python = python::Python;
static NODE: node::Node = node::Node;
static SYSTEM: system::System = system::System;
static FAIL: fail::Fail = fail::Fail;
static DOCKER: docker::Docker = docker::Docker;
static DOCKER_IMAGE: docker_image::DockerImage = docker_image::DockerImage;
static SCRIPT: script::Script = script::Script;
static UNIMPLEMENTED: Unimplemented = Unimplemented;

trait LanguageImpl {
    async fn install(&self, hook: Arc<Hook>, store: &Store) -> Result<InstalledHook>;
    async fn check_health(&self) -> Result<()>;
    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        store: &Store,
    ) -> Result<(i32, Vec<u8>)>;
}

#[derive(thiserror::Error, Debug)]
#[error("Language `{0}` is not implemented yet")]
struct UnimplementedError(String);

struct Unimplemented;

impl LanguageImpl for Unimplemented {
    async fn install(&self, hook: Arc<Hook>, _store: &Store) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        _filenames: &[&String],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        anyhow::bail!(UnimplementedError(format!("{}", hook.language)))
    }
}

// `pre-commit` language support:
// conda: only system version, support env, support additional deps
// coursier: only system version, support env, support additional deps
// dart: only system version, support env, support additional deps
// docker_image: only system version, no env, no additional deps
// docker: only system version, support env, no additional deps
// dotnet: only system version, support env, no additional deps
// fail: only system version, no env, no additional deps
// golang: install requested version, support env, support additional deps
// haskell: only system version, support env, support additional deps
// lua: only system version, support env, support additional deps
// node: install requested version, support env, support additional deps (delegated to nodeenv)
// perl: only system version, support env, support additional deps
// pygrep: only system version, no env, no additional deps
// python: install requested version, support env, support additional deps (delegated to virtualenv)
// r: only system version, support env, support additional deps
// ruby: install requested version, support env, support additional deps (delegated to rbenv)
// rust: install requested version, support env, support additional deps (delegated to rustup and cargo)
// script: only system version, no env, no additional deps
// swift: only system version, support env, no additional deps
// system: only system version, no env, no additional deps

impl Language {
    pub fn supported(lang: Language) -> bool {
        matches!(
            lang,
            Self::Golang
                | Self::Python
                | Self::Node
                | Self::System
                | Self::Fail
                | Self::Docker
                | Self::DockerImage
                | Self::Script
        )
    }

    pub fn supports_install_env(self) -> bool {
        !matches!(
            self,
            Self::DockerImage | Self::Fail | Self::Pygrep | Self::Script | Self::System
        )
    }

    /// Return whether the language allows specifying the version, e.g. we can install a specific
    /// requested language version.
    /// See <https://pre-commit.com/#overriding-language-version>
    pub fn supports_language_version(self) -> bool {
        matches!(
            self,
            Self::Python | Self::Node | Self::Ruby | Self::Rust | Self::Golang
        )
    }

    /// Whether the language supports installing dependencies.
    ///
    /// For example, Python and Node.js support installing dependencies, while
    /// System and Fail do not.
    pub fn supports_dependency(self) -> bool {
        !matches!(
            self,
            Self::DockerImage
                | Self::Fail
                | Self::Pygrep
                | Self::Script
                | Self::System
                | Self::Docker
                | Self::Dotnet
                | Self::Swift
        )
    }

    pub async fn install(&self, hook: Arc<Hook>, store: &Store) -> Result<InstalledHook> {
        match self {
            Self::Golang => GOLANG.install(hook, store).await,
            Self::Python => PYTHON.install(hook, store).await,
            Self::Node => NODE.install(hook, store).await,
            Self::System => SYSTEM.install(hook, store).await,
            Self::Fail => FAIL.install(hook, store).await,
            Self::Docker => DOCKER.install(hook, store).await,
            Self::DockerImage => DOCKER_IMAGE.install(hook, store).await,
            Self::Script => SCRIPT.install(hook, store).await,
            _ => UNIMPLEMENTED.install(hook, store).await,
        }
    }

    pub async fn check_health(&self) -> Result<()> {
        match self {
            Self::Golang => GOLANG.check_health().await,
            Self::Python => PYTHON.check_health().await,
            Self::Node => NODE.check_health().await,
            Self::System => SYSTEM.check_health().await,
            Self::Fail => FAIL.check_health().await,
            Self::Docker => DOCKER.check_health().await,
            Self::DockerImage => DOCKER_IMAGE.check_health().await,
            Self::Script => SCRIPT.check_health().await,
            _ => UNIMPLEMENTED.check_health().await,
        }
    }

    pub async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        // fast path for hooks implemented in Rust
        if builtin::check_fast_path(hook) {
            return builtin::run_fast_path(hook, filenames).await;
        }

        match self {
            Self::Golang => GOLANG.run(hook, filenames, store).await,
            Self::Python => PYTHON.run(hook, filenames, store).await,
            Self::Node => NODE.run(hook, filenames, store).await,
            Self::System => SYSTEM.run(hook, filenames, store).await,
            Self::Fail => FAIL.run(hook, filenames, store).await,
            Self::Docker => DOCKER.run(hook, filenames, store).await,
            Self::DockerImage => DOCKER_IMAGE.run(hook, filenames, store).await,
            Self::Script => SCRIPT.run(hook, filenames, store).await,
            _ => UNIMPLEMENTED.run(hook, filenames, store).await,
        }
    }
}

/// Create a symlink or copy the file on Windows.
/// Tries symlink first, falls back to copy if symlink fails.
async fn create_symlink_or_copy(source: &Path, target: &Path) -> Result<()> {
    if target.exists() {
        fs_err::tokio::remove_file(target).await?;
    }

    #[cfg(not(windows))]
    {
        // Try symlink on Unix systems
        match fs_err::tokio::symlink(source, target).await {
            Ok(()) => {
                trace!(
                    "Created symlink from {} to {}",
                    source.display(),
                    target.display()
                );
                return Ok(());
            }
            Err(e) => {
                trace!(
                    "Failed to create symlink from {} to {}: {}",
                    source.display(),
                    target.display(),
                    e
                );
            }
        }
    }

    #[cfg(windows)]
    {
        // Try Windows symlink API (requires admin privileges)
        use std::os::windows::fs::symlink_file;
        match symlink_file(source, target) {
            Ok(()) => {
                trace!(
                    "Created Windows symlink from {} to {}",
                    source.display(),
                    target.display()
                );
                return Ok(());
            }
            Err(e) => {
                trace!(
                    "Failed to create Windows symlink from {} to {}: {}",
                    source.display(),
                    target.display(),
                    e
                );
            }
        }
    }

    // Fallback to copy
    trace!(
        "Falling back to copy from {} to {}",
        source.display(),
        target.display()
    );
    fs_err::tokio::copy(source, target).await.with_context(|| {
        format!(
            "Failed to copy file from {} to {}",
            source.display(),
            target.display(),
        )
    })?;

    Ok(())
}

async fn download_and_extract(
    client: &reqwest::Client,
    url: &str,
    target: &Path,
    filename: &str,
    scratch: &Path,
) -> Result<()> {
    let response = client
        .get(url)
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

    let tarball = response
        .bytes_stream()
        .map_err(std::io::Error::other)
        .into_async_read()
        .compat();

    let temp_dir = tempfile::tempdir_in(scratch)?;
    trace!(url = %url, temp_dir = ?temp_dir.path(), "Downloading");

    let ext = ArchiveExtension::from_path(filename)?;
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

    trace!(temp_dir = ?extracted, target = %target.display(), "Moving to target");
    // TODO: retry on Windows
    fs_err::tokio::rename(extracted, target).await?;

    Ok(())
}
