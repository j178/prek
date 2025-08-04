use std::collections::HashMap;
use std::sync::Arc;

use anyhow::Result;

use crate::builtin;
use crate::config::Language;
use crate::hook::{Hook, InstalledHook};
use crate::store::Store;

mod docker;
mod docker_image;
mod fail;
mod node;
mod python;
mod script;
mod system;
pub mod version;

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
        env_vars: &HashMap<&'static str, String>,
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
        _env_vars: &HashMap<&'static str, String>,
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
            Self::Python
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
        env_vars: &HashMap<&'static str, String>,
        store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        // fast path for hooks implemented in Rust
        if builtin::check_fast_path(hook) {
            return builtin::run_fast_path(hook, filenames, env_vars).await;
        }

        match self {
            Self::Python => PYTHON.run(hook, filenames, env_vars, store).await,
            Self::Node => NODE.run(hook, filenames, env_vars, store).await,
            Self::System => SYSTEM.run(hook, filenames, env_vars, store).await,
            Self::Fail => FAIL.run(hook, filenames, env_vars, store).await,
            Self::Docker => DOCKER.run(hook, filenames, env_vars, store).await,
            Self::DockerImage => DOCKER_IMAGE.run(hook, filenames, env_vars, store).await,
            Self::Script => SCRIPT.run(hook, filenames, env_vars, store).await,
            _ => UNIMPLEMENTED.run(hook, filenames, env_vars, store).await,
        }
    }
}
