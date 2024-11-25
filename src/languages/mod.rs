use std::any::type_name;
use std::collections::HashMap;
use std::fmt::Display;
use std::sync::Arc;

use crate::config;
use crate::hook::Hook;
use anyhow::Result;
use enum_dispatch::enum_dispatch;

mod docker;
mod docker_image;
mod fail;
mod node;
mod python;
mod system;

pub const DEFAULT_VERSION: &str = "default";

#[enum_dispatch(Language)]
pub trait LanguageImpl {
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

#[enum_dispatch]
#[derive(Debug, Copy, Clone)]
pub enum Language {
    Python(python::Python),
    Node(node::Node),
    System(system::System),
    Fail(fail::Fail),
    Docker(docker::Docker),
    DockerImage(docker_image::DockerImage),
}

impl From<config::Language> for Language {
    fn from(language: config::Language) -> Self {
        match language {
            // config::Language::Conda => Language::Conda,
            // config::Language::Coursier => Language::Coursier,
            // config::Language::Dart => Language::Dart,
            config::Language::Docker => Language::Docker(docker::Docker),
            config::Language::DockerImage => Language::DockerImage(docker_image::DockerImage),
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
        let l = type_name::<Self>();
        f.write_str(l)
    }
}
