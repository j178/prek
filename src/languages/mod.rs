use std::collections::HashMap;
use std::sync::Arc;

use crate::builtin;
use crate::config::Language;
use crate::hook::Hook;
use anyhow::Result;

mod docker;
mod docker_image;
mod fail;
mod node;
mod python;
mod system;

static PYTHON: python::Python = python::Python;
static NODE: node::Node = node::Node;
static SYSTEM: system::System = system::System;
static FAIL: fail::Fail = fail::Fail;
static DOCKER: docker::Docker = docker::Docker;
static DOCKER_IMAGE: docker_image::DockerImage = docker_image::DockerImage;

pub const DEFAULT_VERSION: &str = "default";

trait LanguageImpl {
    fn default_version(&self) -> &str;
    fn environment_dir(&self) -> Option<&str>;
    async fn install(&self, hook: &Hook) -> Result<()>;
    async fn check_health(&self) -> Result<()>;
    async fn run(
        &self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: Arc<HashMap<&'static str, String>>,
    ) -> Result<(i32, Vec<u8>)>;
}

impl Language {
    pub fn default_version(&self) -> &str {
        match self {
            Self::Python => PYTHON.default_version(),
            Self::Node => NODE.default_version(),
            Self::System => SYSTEM.default_version(),
            Self::Fail => FAIL.default_version(),
            Self::Docker => DOCKER.default_version(),
            Self::DockerImage => DOCKER_IMAGE.default_version(),
            _ => todo!(),
        }
    }

    pub fn environment_dir(&self) -> Option<&str> {
        match self {
            Self::Python => PYTHON.environment_dir(),
            Self::Node => NODE.environment_dir(),
            Self::System => SYSTEM.environment_dir(),
            Self::Fail => FAIL.environment_dir(),
            Self::Docker => DOCKER.environment_dir(),
            Self::DockerImage => DOCKER_IMAGE.environment_dir(),
            _ => todo!(),
        }
    }

    pub async fn install(&self, hook: &Hook) -> Result<()> {
        match self {
            Self::Python => PYTHON.install(hook).await,
            Self::Node => NODE.install(hook).await,
            Self::System => SYSTEM.install(hook).await,
            Self::Fail => FAIL.install(hook).await,
            Self::Docker => DOCKER.install(hook).await,
            Self::DockerImage => DOCKER_IMAGE.install(hook).await,
            _ => todo!(),
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
            _ => todo!(),
        }
    }

    pub async fn run(
        &self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: Arc<HashMap<&'static str, String>>,
    ) -> Result<(i32, Vec<u8>)> {
        // fast path for hooks implemented in Rust
        if builtin::check_fast_path(hook) {
            return builtin::run_fast_path(hook, filenames, env_vars).await;
        }

        match self {
            Self::Python => PYTHON.run(hook, filenames, env_vars).await,
            Self::Node => NODE.run(hook, filenames, env_vars).await,
            Self::System => SYSTEM.run(hook, filenames, env_vars).await,
            Self::Fail => FAIL.run(hook, filenames, env_vars).await,
            Self::Docker => DOCKER.run(hook, filenames, env_vars).await,
            Self::DockerImage => DOCKER_IMAGE.run(hook, filenames, env_vars).await,
            _ => todo!(),
        }
    }
}
