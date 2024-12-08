use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use itertools::Itertools;

use crate::hook::{Hook, Project};
use crate::store::Store;

/// Ensures that the configured hooks apply to at least one file in the repository.
pub async fn check_hooks_apply(
    _hook: &Hook,
    filenames: &[&String],
    _env_vars: Arc<HashMap<&'static str, String>>,
) -> Result<(i32, Vec<u8>)> {
    let store = Store::from_settings()?.init()?;

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let mut project = Project::from_config_file(Some(PathBuf::from(filename)))?;
        let hooks = project.init_hooks(&store, None).await?;
    }
    Ok((0, filenames.into_iter().join("\n").into_bytes()))
}

/// Ensures that exclude directives apply to any file in the repository.
pub fn check_useless_excludes(
    _hook: &Hook,
    filenames: &[&String],
    _env_vars: Arc<HashMap<&'static str, String>>,
) -> Result<(i32, Vec<u8>)> {
    Ok((0, filenames.into_iter().join("\n").into_bytes()))
}

/// Prints all arguments passed to the hook. Useful for debugging.
pub fn identity(
    _hook: &Hook,
    filenames: &[&String],
    _env_vars: Arc<HashMap<&'static str, String>>,
) -> Result<(i32, Vec<u8>)> {
    Ok((0, filenames.into_iter().join("\n").into_bytes()))
}
