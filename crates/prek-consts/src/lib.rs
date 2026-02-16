pub mod env_vars;

use std::ffi::OsString;
use std::path::Path;

use env_vars::EnvVars;

pub const PRE_COMMIT_CONFIG_YAML: &str = ".pre-commit-config.yaml";
pub const PRE_COMMIT_CONFIG_YML: &str = ".pre-commit-config.yml";
pub const PREK_TOML: &str = "prek.toml";
pub const PRE_COMMIT_HOOKS_YAML: &str = ".pre-commit-hooks.yaml";

pub static CONFIG_FILENAMES: &[&str] = &[PREK_TOML, PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML];

/// Prepend paths to the current $PATH, returning the joined result.
///
/// The resulting `OsString` can be used to set the `PATH` environment variable.
pub fn prepend_paths(paths: &[&Path]) -> Result<OsString, std::env::JoinPathsError> {
    std::env::join_paths(
        paths.iter().map(|p| p.to_path_buf()).chain(
            EnvVars::var_os(EnvVars::PATH)
                .as_ref()
                .iter()
                .flat_map(std::env::split_paths),
        ),
    )
}
