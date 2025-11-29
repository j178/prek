use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::TryStreamExt;
use prek_consts::env_vars::EnvVars;
use tokio_util::compat::FuturesAsyncReadCompatExt;
use tracing::{debug, error, instrument, trace};

use crate::archive::ArchiveExtension;
use crate::cli::reporter::HookInstallReporter;
use crate::config::Language;
use crate::fs::{CWD, Simplified};
use crate::hook::{Hook, InstallInfo, InstalledHook, Repo};
use crate::identify::parse_shebang;
use crate::store::Store;
use crate::{archive, hooks, warn_user_once};

mod docker;
mod docker_image;
mod fail;
mod golang;
mod lua;
mod node;
mod pygrep;
mod python;
mod ruby;
mod rust;
mod script;
mod system;
pub mod version;

static DOCKER: docker::Docker = docker::Docker;
static DOCKER_IMAGE: docker_image::DockerImage = docker_image::DockerImage;
static FAIL: fail::Fail = fail::Fail;
static GOLANG: golang::Golang = golang::Golang;
static LUA: lua::Lua = lua::Lua;
static NODE: node::Node = node::Node;
static PYGREP: pygrep::Pygrep = pygrep::Pygrep;
static PYTHON: python::Python = python::Python;
static RUBY: ruby::Ruby = ruby::Ruby;
static RUST: rust::Rust = rust::Rust;
static SCRIPT: script::Script = script::Script;
static SYSTEM: system::System = system::System;
static UNIMPLEMENTED: Unimplemented = Unimplemented;

trait LanguageImpl {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook>;

    async fn check_health(&self, info: &InstallInfo) -> Result<()>;

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
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

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        _filenames: &[&Path],
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
                | Self::Docker
                | Self::DockerImage
                | Self::Fail
                | Self::Lua
                | Self::Node
                | Self::Pygrep
                | Self::Python
                | Self::Ruby
                | Self::Rust
                | Self::Script
                | Self::System
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
            Self::Python | Self::Node | Self::Golang | Self::Ruby | Self::Rust
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
            Self::Docker => DOCKER.install(hook, store, reporter).await,
            Self::DockerImage => DOCKER_IMAGE.install(hook, store, reporter).await,
            Self::Fail => FAIL.install(hook, store, reporter).await,
            Self::Golang => GOLANG.install(hook, store, reporter).await,
            Self::Lua => LUA.install(hook, store, reporter).await,
            Self::Node => NODE.install(hook, store, reporter).await,
            Self::Pygrep => PYGREP.install(hook, store, reporter).await,
            Self::Python => PYTHON.install(hook, store, reporter).await,
            Self::Ruby => RUBY.install(hook, store, reporter).await,
            Self::Rust => RUST.install(hook, store, reporter).await,
            Self::Script => SCRIPT.install(hook, store, reporter).await,
            Self::System => SYSTEM.install(hook, store, reporter).await,
            _ => UNIMPLEMENTED.install(hook, store, reporter).await,
        }
    }

    pub async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        match self {
            Self::Docker => DOCKER.check_health(info).await,
            Self::DockerImage => DOCKER_IMAGE.check_health(info).await,
            Self::Fail => FAIL.check_health(info).await,
            Self::Golang => GOLANG.check_health(info).await,
            Self::Lua => LUA.check_health(info).await,
            Self::Node => NODE.check_health(info).await,
            Self::Pygrep => PYGREP.check_health(info).await,
            Self::Python => PYTHON.check_health(info).await,
            Self::Ruby => RUBY.check_health(info).await,
            Self::Rust => RUST.check_health(info).await,
            Self::Script => SCRIPT.check_health(info).await,
            Self::System => SYSTEM.check_health(info).await,
            _ => UNIMPLEMENTED.check_health(info).await,
        }
    }

    #[instrument(level = "trace", skip_all, fields(hook_id = %hook.id, language = %hook.language))]
    pub async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        match hook.repo() {
            Repo::Meta { .. } => {
                return hooks::MetaHooks::from_str(&hook.id)
                    .unwrap()
                    .run(store, hook, filenames)
                    .await;
            }
            Repo::Builtin { .. } => {
                return hooks::BuiltinHooks::from_str(&hook.id)
                    .unwrap()
                    .run(store, hook, filenames)
                    .await;
            }
            Repo::Remote { .. } => {
                // Fast path for hooks implemented in Rust
                if hooks::check_fast_path(hook) {
                    return hooks::run_fast_path(store, hook, filenames).await;
                }
            }
            Repo::Local { .. } => {}
        }

        match self {
            Self::Docker => DOCKER.run(hook, filenames, store).await,
            Self::DockerImage => DOCKER_IMAGE.run(hook, filenames, store).await,
            Self::Fail => FAIL.run(hook, filenames, store).await,
            Self::Golang => GOLANG.run(hook, filenames, store).await,
            Self::Lua => LUA.run(hook, filenames, store).await,
            Self::Node => NODE.run(hook, filenames, store).await,
            Self::Pygrep => PYGREP.run(hook, filenames, store).await,
            Self::Python => PYTHON.run(hook, filenames, store).await,
            Self::Ruby => RUBY.run(hook, filenames, store).await,
            Self::Rust => RUST.run(hook, filenames, store).await,
            Self::Script => SCRIPT.run(hook, filenames, store).await,
            Self::System => SYSTEM.run(hook, filenames, store).await,
            _ => UNIMPLEMENTED.run(hook, filenames, store).await,
        }
    }
}

/// Try to extract metadata from the given hook entry if possible.
pub(crate) async fn extract_metadata_from_entry(hook: &mut Hook) -> Result<()> {
    match hook.language {
        Language::Python => python::extract_pep723_metadata(hook).await,
        Language::Golang => golang::extract_go_mod_metadata(hook).await,
        _ => Ok(()),
    }
}

/// Resolve the actual process invocation, honoring shebangs and PATH lookups.
pub(crate) fn resolve_command(mut cmds: Vec<String>, paths: Option<&OsStr>) -> Vec<String> {
    let candidate = &cmds[0];
    let resolved_binary = match which::which_in(candidate, paths, &*CWD) {
        Ok(p) => p,
        Err(_) => PathBuf::from(candidate),
    };
    trace!("Resolved command: {}", resolved_binary.display());

    if let Ok(mut shebang_argv) = parse_shebang(&resolved_binary) {
        trace!("Found shebang: {:?}", shebang_argv);
        // Resolve the interpreter path, convert "python3" to "python3.exe" on Windows
        if let Ok(p) = which::which_in(&shebang_argv[0], paths, &*CWD) {
            shebang_argv[0] = p.to_string_lossy().to_string();
            trace!("Resolved interpreter: {}", &shebang_argv[0]);
        }
        shebang_argv.push(resolved_binary.to_string_lossy().to_string());
        shebang_argv.extend_from_slice(&cmds[1..]);
        shebang_argv
    } else {
        cmds[0] = resolved_binary.to_string_lossy().to_string();
        cmds
    }
}

async fn download_and_extract(
    url: &str,
    filename: &str,
    store: &Store,
    callback: impl AsyncFn(&Path) -> Result<()>,
) -> Result<()> {
    let response = REQWEST_CLIENT
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

    let scratch_dir = store.scratch_path();
    let temp_dir = tempfile::tempdir_in(&scratch_dir)?;
    debug!(url = %url, temp_dir = ?temp_dir.path(), "Downloading");

    let ext = ArchiveExtension::from_path(filename)?;
    archive::unpack(tarball, ext, temp_dir.path()).await?;

    let extracted = match archive::strip_component(temp_dir.path()) {
        Ok(top_level) => top_level,
        Err(archive::Error::NonSingularArchive(_)) => temp_dir.path().to_path_buf(),
        Err(err) => return Err(err.into()),
    };

    callback(&extracted).await?;

    drop(temp_dir);

    Ok(())
}

pub(crate) static REQWEST_CLIENT: std::sync::LazyLock<reqwest::Client> =
    std::sync::LazyLock::new(|| {
        let native_tls = use_native_tls();
        create_reqwest_client(native_tls)
    });

fn create_reqwest_client(native_tls: bool) -> reqwest::Client {
    let builder = reqwest::ClientBuilder::new()
        .user_agent(format!("prek/{}", crate::version::version()))
        .tls_built_in_root_certs(false);
    let builder = if native_tls {
        debug!("Using native TLS for reqwest client");
        builder.tls_built_in_native_certs(true)
    } else {
        builder.tls_built_in_webpki_certs(true)
    };
    builder.build().unwrap_or_else(|e| {
        error!(
            "Unable to create reqwest client, falling back to default {:?}",
            e
        );
        reqwest::Client::new()
    })
}

fn use_native_tls() -> bool {
    if let Some(val) = EnvVars::var_as_bool(EnvVars::PREK_NATIVE_TLS) {
        return val;
    }

    // SSL_CERT_FILE is only respected when using native TLS
    EnvVars::var_os(EnvVars::SSL_CERT_FILE).is_some_and(|path| {
        let path_exists = Path::new(&path).exists();
        if !path_exists {
            warn_user_once!(
                "Ignoring invalid `SSL_CERT_FILE`. File does not exist: {}.",
                path.simplified_display().cyan()
            );
        }
        path_exists
    })
}

#[cfg(test)]
mod tests {
    #[tokio::test]
    async fn test_native_tls() {
        let client = super::create_reqwest_client(true);
        let resp = client.get("https://github.com").send().await;
        assert!(resp.is_ok(), "Failed to send request with native TLS");
    }
}
