use std::io::Write;
use std::path::PathBuf;

use anyhow::Result;
use fancy_regex::Regex;
use itertools::Itertools;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::cli::run::{CollectOptions, FileFilter, collect_files};
use crate::config::Language;
use crate::hook::Hook;
use crate::store::Store;
use crate::workspace::Project;

/// Ensures that the configured hooks apply to at least one file in the repository.
pub(crate) async fn check_hooks_apply(
    _hook: &Hook,
    filenames: &[&String],
) -> Result<(i32, Vec<u8>)> {
    let store = Store::from_settings()?.init()?;

    let input = collect_files(CollectOptions::default().with_all_files(true)).await?;

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
                writeln!(&mut output, "{} does not apply to this repository", hook.id)?;
            }
        }
    }

    Ok((code, output))
}

// Returns true if the exclude patter matches any files matching the include pattern.
fn excludes_any<T: AsRef<str> + Sync>(
    files: &[T],
    include: Option<&str>,
    exclude: Option<&str>,
) -> Result<bool> {
    if exclude.is_none_or(|s| s == "^$") {
        return Ok(true);
    }

    let include = include.map(Regex::new).transpose()?;
    let exclude = exclude.map(Regex::new).transpose()?;
    Ok(files.into_par_iter().any(|f| {
        let f = f.as_ref();
        if let Some(re) = &include {
            if !re.is_match(f).unwrap_or(false) {
                return false;
            }
        }
        if let Some(re) = &exclude {
            if !re.is_match(f).unwrap_or(false) {
                return false;
            }
        }
        true
    }))
}

/// Ensures that exclude directives apply to any file in the repository.
pub(crate) async fn check_useless_excludes(
    _hook: &Hook,
    filenames: &[&String],
) -> Result<(i32, Vec<u8>)> {
    let store = Store::from_settings()?.init()?;

    let input = collect_files(CollectOptions::default().with_all_files(true)).await?;

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let mut project = Project::from_config_file(Some(PathBuf::from(filename)))?;

        if !excludes_any(&input, None, project.config().exclude.as_deref())? {
            code = 1;
            writeln!(
                &mut output,
                "The global exclude pattern {:?} does not match any files",
                project.config().exclude.as_deref().unwrap_or("")
            )?;
        }

        let hooks = project.init_hooks(&store, None).await?;

        let filter = FileFilter::new(
            &input,
            project.config().files.as_deref(),
            project.config().exclude.as_deref(),
        )?;

        for hook in hooks {
            let filtered_files = filter.by_tag(&hook);
            if !excludes_any(
                &filtered_files,
                hook.files.as_deref(),
                hook.exclude.as_deref(),
            )? {
                code = 1;
                writeln!(
                    &mut output,
                    "The exclude pattern {:?} for {} does not match any files",
                    hook.exclude.as_deref().unwrap_or(""),
                    hook.id
                )?;
            }
        }
    }

    Ok((code, output))
}

/// Prints all arguments passed to the hook. Useful for debugging.
pub fn identity(_hook: &Hook, filenames: &[&String]) -> (i32, Vec<u8>) {
    (0, filenames.iter().join("\n").into_bytes())
}
