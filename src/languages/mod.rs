use std::collections::HashMap;
use std::fmt::{Debug, Display};
use std::sync::Arc;

use anyhow::Result;

use crate::config;
use crate::hook::Hook;

mod fail;
mod node;
mod python;
mod system;

pub const DEFAULT_VERSION: &str = "default";

trait LanguageImpl {
    fn name(&self) -> config::Language;
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

#[derive(Debug, Copy, Clone)]
pub enum Language {
    Python(python::Python),
    Node(node::Node),
    System(system::System),
    Fail(fail::Fail),
}

impl From<config::Language> for Language {
    fn from(language: config::Language) -> Self {
        match language {
            // config::Language::Conda => Language::Conda,
            // config::Language::Coursier => Language::Coursier,
            // config::Language::Dart => Language::Dart,
            // config::Language::Docker => Language::Docker,
            // config::Language::DockerImage => Language::DockerImage,
            // config::Language::Dotnet => Language::Dotnet,
            config::Language::Fail => Language::Fail(fail::Fail),
            // config::Language::Golang => Language::Golang,
            // config::Language::Haskell => Language::Haskell,
            // config::Language::Lua => Language::Lua,
            config::Language::Node => Language::Node(node::Node),
            // config::Language::Perl => Language::Perl,
            config::Language::Python => Language::Python(python::Python),
            // config::Language::R => Language::R,
            // config::Language::Ruby => Language::Ruby,
            // config::Language::Rust => Language::Rust,
            // config::Language::Swift => Language::Swift,
            // config::Language::Pygrep => Language::Pygrep,
            // config::Language::Script => Language::Script,
            config::Language::System => Language::System(system::System),
            _ => todo!("Not implemented yet"),
        }
    }
}

impl Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Python(python) => python.fmt(f),
            Self::Node(node) => node.fmt(f),
            Self::System(system) => system.fmt(f),
            Self::Fail(fail) => fail.fmt(f),
        }
    }
}

impl Language {
    pub fn name(self) -> config::Language {
        match self {
            Self::Python(python) => python.name(),
            Self::Node(node) => node.name(),
            Self::System(system) => system.name(),
            Self::Fail(fail) => fail.name(),
        }
    }

    pub fn default_version(&self) -> &str {
        match self {
            Self::Python(python) => python.default_version(),
            Self::Node(node) => node.default_version(),
            Self::System(system) => system.default_version(),
            Self::Fail(fail) => fail.default_version(),
        }
    }

    pub fn environment_dir(&self) -> Option<&str> {
        match self {
            Self::Python(python) => python.environment_dir(),
            Self::Node(node) => node.environment_dir(),
            Self::System(system) => system.environment_dir(),
            Self::Fail(fail) => fail.environment_dir(),
        }
    }

    pub async fn install(&self, hook: &Hook) -> Result<()> {
        match self {
            Self::Python(python) => python.install(hook).await,
            Self::Node(node) => node.install(hook).await,
            Self::System(system) => system.install(hook).await,
            Self::Fail(fail) => fail.install(hook).await,
        }
    }

    pub async fn check_health(&self) -> Result<()> {
        match self {
            Self::Python(python) => python.check_health().await,
            Self::Node(node) => node.check_health().await,
            Self::System(system) => system.check_health().await,
            Self::Fail(fail) => fail.check_health().await,
        }
    }

    pub async fn run(
        &self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: Arc<HashMap<&'static str, String>>,
    ) -> Result<(i32, Vec<u8>)> {
        match self {
            Self::Python(python) => python.run(hook, filenames, env_vars).await,
            Self::Node(node) => node.run(hook, filenames, env_vars).await,
            Self::System(system) => system.run(hook, filenames, env_vars).await,
            Self::Fail(fail) => fail.run(hook, filenames, env_vars).await,
        }
    }
}
