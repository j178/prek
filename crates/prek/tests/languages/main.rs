#[path = "../common/mod.rs"]
mod common;

mod bun;
mod conda;
mod coursier;
mod dart;
mod deno;
#[cfg(all(feature = "docker", target_os = "linux"))]
mod docker;
#[cfg(all(feature = "docker", target_os = "linux"))]
mod docker_image;
mod dotnet;
mod fail;
mod golang;
mod haskell;
mod julia;
mod lua;
mod node;
mod perl;
mod php;
mod pygrep;
mod python;
mod r;
mod ruby;
mod rust;
mod script;
mod shell;
mod swift;
mod system;
mod unsupported;
