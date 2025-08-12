use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use fancy_regex::Regex;
use itertools::Itertools;
use rayon::iter::{IntoParallelIterator, ParallelIterator};

use crate::cli::run::{CollectOptions, FileFilter, collect_files};
use crate::config::{self, HookOptions, Language};
use crate::hook::Hook;
use crate::store::STORE;
use crate::workspace::Project;

/// Ensures that the configured hooks apply to at least one file in the repository.
pub(crate) async fn check_hooks_apply(
    _hook: &Hook,
    filenames: &[&String],
) -> Result<(i32, Vec<u8>)> {
    let store = STORE.as_ref()?;

    let input = collect_files(CollectOptions::default().with_all_files(true)).await?;

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let mut project = Project::from_config_file(PathBuf::from(filename))?;
        let hooks = project.init_hooks(store, None).await?;

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

// Returns true if the exclude pattern matches any files matching the include pattern.
fn excludes_any<T: AsRef<str> + Sync>(
    files: &[T],
    include: Option<&Regex>,
    exclude: Option<&Regex>,
) -> bool {
    if exclude.is_none() {
        return true;
    }

    files.into_par_iter().any(|f| {
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
    })
}

/// Ensures that exclude directives apply to any file in the repository.
pub(crate) async fn check_useless_excludes(
    _hook: &Hook,
    filenames: &[&String],
) -> Result<(i32, Vec<u8>)> {
    let input = collect_files(CollectOptions::default().with_all_files(true)).await?;

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let config = config::read_config(Path::new(filename))?;

        if !excludes_any(&input, None, config.exclude.as_deref()) {
            code = 1;
            writeln!(
                &mut output,
                "The global exclude pattern `{}` does not match any files",
                config.exclude.as_deref().map_or("", |r| r.as_str())
            )?;
        }

        let filter = FileFilter::new(&input, config.files.as_ref(), config.exclude.as_ref());

        for repo in &config.repos {
            let hooks_iter: Box<dyn Iterator<Item = (&String, &HookOptions)>> = match repo {
                config::Repo::Remote(r) => Box::new(r.hooks.iter().map(|h| (&h.id, &h.options))),
                config::Repo::Local(r) => Box::new(r.hooks.iter().map(|h| (&h.id, &h.options))),
                config::Repo::Meta(r) => Box::new(r.hooks.iter().map(|h| (&h.0.id, &h.0.options))),
            };

            for (hook_id, opts) in hooks_iter {
                let filtered_files = filter.by_type(
                    opts.types.as_deref().unwrap_or(&[]),
                    opts.types_or.as_deref().unwrap_or(&[]),
                    opts.exclude_types.as_deref().unwrap_or(&[]),
                );

                if !excludes_any(
                    &filtered_files,
                    opts.files.as_deref(),
                    opts.exclude.as_deref(),
                ) {
                    code = 1;
                    writeln!(
                        &mut output,
                        "The exclude pattern `{}` for `{hook_id}` does not match any files",
                        opts.exclude.as_deref().map_or("", |r| r.as_str())
                    )?;
                }
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
    fn test_excludes_any() {
        let files = vec!["file1.txt", "file2.txt", "file3.txt"];
        assert!(excludes_any(
            &files,
            Regex::new(r"file.*").ok().as_ref(),
            Regex::new(r"file2\.txt").ok().as_ref()
        ));
        assert!(!excludes_any(
            &files,
            Regex::new(r"file.*").ok().as_ref(),
            Regex::new(r"file4\.txt").ok().as_ref()
        ));
        assert!(excludes_any(&files, None, None));

        let files = vec!["html/file1.html", "html/file2.html"];
        assert!(excludes_any(
            &files,
            None,
            Regex::new(r"^html/").ok().as_ref()
        ));
    }
}
