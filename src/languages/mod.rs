use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::TryStreamExt;
use http::header::USER_AGENT;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{debug, trace};

use crate::archive::ArchiveExtension;
use crate::cli::reporter::HookInstallReporter;
use crate::config::Language;
use crate::fs::CWD;
use crate::hook::{Hook, InstalledHook};
use crate::identify::parse_shebang;
use crate::store::{STORE, Store};
use crate::version::version;
use crate::{archive, builtin};

mod docker;
mod docker_image;
mod fail;
mod golang;
mod node;
mod pygrep;
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
static PYGREP: pygrep::Pygrep = pygrep::Pygrep;
static UNIMPLEMENTED: Unimplemented = Unimplemented;

trait LanguageImpl {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook>;
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
    async fn install(
        &self,
        hook: Arc<Hook>,
        _store: &Store,
        _reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
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
                | Self::Pygrep
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

    pub async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        match self {
            Self::Golang => GOLANG.install(hook, store, reporter).await,
            Self::Python => PYTHON.install(hook, store, reporter).await,
            Self::Node => NODE.install(hook, store, reporter).await,
            Self::System => SYSTEM.install(hook, store, reporter).await,
            Self::Fail => FAIL.install(hook, store, reporter).await,
            Self::Docker => DOCKER.install(hook, store, reporter).await,
            Self::DockerImage => DOCKER_IMAGE.install(hook, store, reporter).await,
            Self::Script => SCRIPT.install(hook, store, reporter).await,
            Self::Pygrep => PYGREP.install(hook, store, reporter).await,
            _ => UNIMPLEMENTED.install(hook, store, reporter).await,
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
            Self::Pygrep => PYGREP.check_health().await,
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
            Self::Pygrep => PYGREP.run(hook, filenames, store).await,
            _ => UNIMPLEMENTED.run(hook, filenames, store).await,
        }
    }
}

/// Try to extract metadata from the given hook entry if possible.
///
/// Currently, only PEP 723 inline metadata for `python` hooks is supported.
/// First part of `entry` must be a file path to the Python script.
/// Effectively, we are implementing a new `python-script` language which works like `script`.
/// But we don't want to introduce a new language just for this for now.
pub(crate) async fn extract_metadata_from_entry(hook: &mut Hook) -> Result<()> {
    // Only support `python` hooks for now.
    if hook.language == Language::Python {
        return python::extract_pep723_metadata(hook).await;
    }

    Ok(())
}

pub(crate) fn resolve_command(mut cmds: Vec<String>, env_path: Option<&OsStr>) -> Vec<String> {
    let cmd = &cmds[0];
    let exe_path = match which::which_in(cmd, env_path, &*CWD) {
        Ok(p) => p,
        Err(_) => PathBuf::from(cmd),
    };
    trace!("Resolved command: {}", exe_path.display());

    if let Ok(mut interpreter) = parse_shebang(&exe_path) {
        trace!("Found shebang: {:?}", interpreter);
        // Resolve the interpreter path, convert "python3" to "python3.exe" on Windows
        if let Ok(p) = which::which_in(&interpreter[0], env_path, &*CWD) {
            interpreter[0] = p.to_string_lossy().to_string();
            trace!("Resolved interpreter: {}", &interpreter[0]);
        }
        interpreter.push(exe_path.to_string_lossy().to_string());
        interpreter.extend_from_slice(&cmds[1..]);
        interpreter
    } else {
        cmds[0] = exe_path.to_string_lossy().to_string();
        cmds
    }
}

async fn download_and_extract(
    client: &reqwest::Client,
    url: &str,
    filename: &str,
    callback: impl AsyncFn(&Path) -> Result<()>,
) -> Result<()> {
    let response = client
        .get(url)
        .header(USER_AGENT, format!("prek/{}", version()))
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

    let scratch = STORE.as_ref()?.scratch_path();
    fs_err::tokio::create_dir_all(&scratch).await?;

    let temp_dir = tempfile::tempdir_in(&scratch)?;
    debug!(url = %url, temp_dir = ?temp_dir.path(), "Downloading");

    let ext = ArchiveExtension::from_path(filename)?;
    archive::unpack(tarball, ext, temp_dir.path()).await?;

    let extracted = match archive::strip_component(temp_dir.path()) {
        Ok(top_level) => top_level,
        Err(archive::Error::NonSingularArchive(_)) => temp_dir.path().to_path_buf(),
        Err(err) => return Err(err.into()),
    };

    callback(&extracted).await?;

    fs_err::tokio::remove_dir_all(temp_dir.path()).await?;

    Ok(())
}
