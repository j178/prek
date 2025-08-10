use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use fancy_regex::Regex;
use itertools::Itertools;
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use tracing::error;

use crate::cli::run::{CollectOptions, FileFilter, FileTagFilter, FilenameFilter, collect_files};
use crate::config::Language;
use crate::hook::Hook;
use crate::identify::tags_from_path;
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
            project.config().files.as_ref(),
            project.config().exclude.as_ref(),
        );

        for hook in hooks {
            if hook.always_run || matches!(hook.language, Language::Fail) {
                continue;
            }

            let filenames = filter.for_hook(&hook);

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
    let Some(exclude_s) = exclude else {
        // An empty/None exclude pattern is always "useful" according to pre-commit.
        return Ok(true);
    };
    if exclude_s == "^$" {
        // This is the default exclude pattern, which is also considered "useful".
        return Ok(true);
    }

    let include_re = include.map(Regex::new).transpose()?;
    let exclude_re = Regex::new(exclude_s)?;

    Ok(files.into_par_iter().any(|f| {
        let f = f.as_ref();

        // Check if included
        if let Some(re) = &include_re {
            if !re.is_match(f).unwrap_or(false) {
                return false; // Not included, so exclude pattern is irrelevant for this file
            }
        }

        // Check if excluded
        exclude_re.is_match(f).unwrap_or(false)
    }))
}

/// Ensures that exclude directives apply to any file in the repository.
pub(crate) async fn check_useless_excludes(
    _hook: &Hook,
    filenames: &[&String],
) -> Result<(i32, Vec<u8>)> {
    let input = collect_files(CollectOptions::default().with_all_files(true)).await?;
    let store = Store::from_settings()?.init()?;

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let mut project = Project::from_config_file(Some(PathBuf::from(filename)))?;
        let hooks = project.init_hooks(&store, None).await?;
        let config = project.config();

        if !excludes_any(&input, None, config.exclude.as_ref().map(|r| r.as_str()))? {
            code = 1;
            writeln!(
                &mut output,
                "The global exclude pattern {:?} does not match any files",
                config.exclude.as_ref().map_or("", |r| r.as_str())
            )?;
        }

        let filter = FileFilter::new(&input, config.files.as_ref(), config.exclude.as_ref());

        for hook in &hooks {
            // Get files that pass the hook's `files` regex, from the globally-filtered list.
            let filename_filter = FilenameFilter::new(hook.files.as_deref(), None);
            let files_after_pattern_filter: Vec<_> = filter
                .filenames
                .par_iter()
                .filter(|f| filename_filter.filter(f))
                .copied()
                .collect();

            // Then, get files that pass the hook's `types` filter.
            let tag_filter = FileTagFilter::for_hook(hook);
            let applicable_files: Vec<_> = files_after_pattern_filter
                .par_iter()
                .filter(|filename| {
                    let path = Path::new(filename);
                    match tags_from_path(path) {
                        Ok(tags) => tag_filter.filter(&tags),
                        Err(err) => {
                            error!(filename = %filename, error = %err, "Failed to get tags");
                            false
                        }
                    }
                })
                .map(|f| f.as_str())
                .collect();

            if !excludes_any(
                &applicable_files,
                None, // Already filtered by hook's include pattern.
                hook.exclude.as_ref().map(|r| r.as_str()),
            )? {
                code = 1;
                writeln!(
                    &mut output,
                    "The exclude pattern {:?} for {} does not match any files",
                    hook.exclude.as_ref().map_or("", |r| r.as_str()),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_excludes_any() -> Result<()> {
        let files = vec!["file1.txt", "file2.txt", "file3.txt"];
        assert!(excludes_any(&files, Some(r"file.*"), Some(r"file2\.txt"))?);
        assert!(!excludes_any(&files, Some(r"file.*"), Some(r"file4\.txt"))?);
        assert!(excludes_any(&files, None, None)?);

        let files = vec!["html/file1.html", "html/file2.html"];
        assert!(excludes_any(&files, None, Some(r"^html/"))?);
        Ok(())
    }
}
