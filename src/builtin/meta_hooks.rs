use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use itertools::Itertools;

use crate::config::Language;
use crate::hook::{Hook, Project};
use crate::run::{all_filenames, FileFilter, FileOptions};
use crate::store::Store;

/// Ensures that the configured hooks apply to at least one file in the repository.
pub async fn check_hooks_apply(
    _hook: &Hook,
    filenames: &[&String],
    _env_vars: Arc<HashMap<&'static str, String>>,
) -> Result<(i32, Vec<u8>)> {
    let store = Store::from_settings()?.init()?;

    let input = all_filenames(FileOptions::default().with_all_files(true)).await?;

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let mut project = Project::from_config_file(Some(PathBuf::from(filename)))?;
        let hooks = project.init_hooks(&store, None).await?;

        let filter = FileFilter::new(
            &input,
            project.config().files.as_deref(),
            project.config().exclude.as_deref(),
        )?;

        for hook in hooks {
            if hook.always_run || matches!(hook.language, Language::Fail) {
                continue;
            }

            let filenames = filter.for_hook(&hook)?;

            if filenames.is_empty() {
                code = 1;
                output
                    .extend(format!("{} does not apply to this repository\n", hook.id).as_bytes());
            }
        }
    }

    Ok((code, output))
}

fn excludes_any(files: &[String], include: &str, exclude: &str) -> bool {
    if exclude == "^$" {
        return true;
    }

    true
}

/// Ensures that exclude directives apply to any file in the repository.
pub async fn check_useless_excludes(
    _hook: &Hook,
    filenames: &[&String],
    _env_vars: Arc<HashMap<&'static str, String>>,
) -> Result<(i32, Vec<u8>)> {
    let store = Store::from_settings()?.init()?;

    let input = all_filenames(FileOptions::default().with_all_files(true)).await?;

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let mut project = Project::from_config_file(Some(PathBuf::from(filename)))?;
        let hooks = project.init_hooks(&store, None).await?;

        let filter = FileFilter::new(
            &input,
            project.config().files.as_deref(),
            project.config().exclude.as_deref(),
        )?;

        for hook in hooks {
            if hook.always_run || matches!(hook.language, Language::Fail) {
                continue;
            }

            let filenames = filter.for_hook(&hook)?;

            if filenames.len() == input.len() {
                code = 1;
                output.extend(
                    format!("{} excludes all files in the repository\n", hook.id).as_bytes(),
                );
            }
        }
    }

    Ok((code, output))
}

/// Prints all arguments passed to the hook. Useful for debugging.
pub fn identity(
    _hook: &Hook,
    filenames: &[&String],
    _env_vars: Arc<HashMap<&'static str, String>>,
) -> (i32, Vec<u8>) {
    (0, filenames.iter().join("\n").into_bytes())
}
