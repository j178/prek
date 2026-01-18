pub mod env_vars;

use std::ffi::OsString;
use std::path::Path;

use env_vars::EnvVars;

pub const CONFIG_FILE: &str = ".pre-commit-config.yaml";
pub const ALT_CONFIG_FILE: &str = ".pre-commit-config.yml";
pub const MANIFEST_FILE: &str = ".pre-commit-hooks.yaml";

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
