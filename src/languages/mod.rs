use std::collections::HashMap;

use anyhow::Result;

use crate::builtin;
use crate::config::Language;
use crate::hook::{Hook, ResolvedHook};
use crate::store::Store;

mod docker;
mod docker_image;
mod fail;
mod node;
mod python;
mod script;
mod system;

static PYTHON: python::Python = python::Python;
static NODE: node::Node = node::Node;
static SYSTEM: system::System = system::System;
static FAIL: fail::Fail = fail::Fail;
static DOCKER: docker::Docker = docker::Docker;
static DOCKER_IMAGE: docker_image::DockerImage = docker_image::DockerImage;
static SCRIPT: script::Script = script::Script;

trait LanguageImpl {
    /// Whether the language supports installing dependencies.
    ///
    /// For example, Python and Node.js support installing dependencies, while
    /// System and Fail do not.
    fn supports_dependency(&self) -> bool;
    async fn resolve(&self, hook: &Hook, store: &Store) -> Result<ResolvedHook>;
    async fn install(&self, hook: &ResolvedHook, store: &Store) -> Result<()>;
    async fn check_health(&self) -> Result<()>;
    async fn run(
        &self,
        hook: &ResolvedHook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
        store: &Store,
    ) -> Result<(i32, Vec<u8>)>;
}

impl Language {
    /// Return whether the language allows specifying the version.
    /// See <https://pre-commit.com/#overriding-language-version>
    pub fn supports_language_version(self) -> bool {
        matches!(
            self,
            Self::Python | Self::Node | Self::Ruby | Self::Rust | Self::Golang
        )
    }

    pub fn supports_dependency(self) -> bool {
        match self {
            Self::Python => PYTHON.supports_dependency(),
            // Self::Node => NODE.supports_dependency(),
            Self::System => SYSTEM.supports_dependency(),
            Self::Fail => FAIL.supports_dependency(),
            Self::Docker => DOCKER.supports_dependency(),
            Self::DockerImage => DOCKER_IMAGE.supports_dependency(),
            Self::Script => SCRIPT.supports_dependency(),
            _ => todo!("{}", self.as_str()),
        }
    }

    pub async fn resolve(&self, hook: &Hook, store: &Store) -> Result<ResolvedHook> {
        match self {
            Self::Python => PYTHON.resolve(hook, store).await,
            // Self::Node => NODE.resolve(hook, store).await,
            Self::System => SYSTEM.resolve(hook, store).await,
            Self::Fail => FAIL.resolve(hook, store).await,
            Self::Docker => DOCKER.resolve(hook, store).await,
            Self::DockerImage => DOCKER_IMAGE.resolve(hook, store).await,
            Self::Script => SCRIPT.resolve(hook, store).await,
            _ => todo!("{}", self.as_str()),
        }
    }

    pub async fn install(&self, hook: &ResolvedHook, store: &Store) -> Result<()> {
        match self {
            Self::Python => PYTHON.install(hook, store).await,
            // Self::Node => NODE.install(hook, store).await,
            Self::System => SYSTEM.install(hook, store).await,
            Self::Fail => FAIL.install(hook, store).await,
            Self::Docker => DOCKER.install(hook, store).await,
            Self::DockerImage => DOCKER_IMAGE.install(hook, store).await,
            Self::Script => SCRIPT.install(hook, store).await,
            _ => todo!("{}", self.as_str()),
        }
    }

    pub async fn check_health(&self) -> Result<()> {
        match self {
            Self::Python => PYTHON.check_health().await,
            // Self::Node => NODE.check_health().await,
            Self::System => SYSTEM.check_health().await,
            Self::Fail => FAIL.check_health().await,
            Self::Docker => DOCKER.check_health().await,
            Self::DockerImage => DOCKER_IMAGE.check_health().await,
            Self::Script => SCRIPT.check_health().await,
            _ => todo!("{}", self.as_str()),
        }
    }

    pub async fn run(
        &self,
        hook: &ResolvedHook,
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
            // Self::Node => NODE.run(hook, filenames, env_vars, store).await,
            Self::System => SYSTEM.run(hook, filenames, env_vars, store).await,
            Self::Fail => FAIL.run(hook, filenames, env_vars, store).await,
            Self::Docker => DOCKER.run(hook, filenames, env_vars, store).await,
            Self::DockerImage => DOCKER_IMAGE.run(hook, filenames, env_vars, store).await,
            Self::Script => SCRIPT.run(hook, filenames, env_vars, store).await,
            _ => todo!("{}", self.as_str()),
        }
    }
}
