use std::io::Write;
use std::path::Path;

use anyhow::Result;
use fancy_regex::Regex;
use itertools::Itertools;

use crate::cli::run::{CollectOptions, FileFilter, collect_files};
use crate::config::{self, HookOptions, Language};
use crate::hook::Hook;
use crate::store::STORE;
use crate::workspace::Project;

// For builtin hooks (meta hooks and builtin pre-commit-hooks), they are not run
// in the project root like other hooks. Instead, they run in the workspace root.
// But the input filenames are all relative to the project root. So when accessing these files,
// we need to adjust the paths by prepending the project relative path.
// When matching files (files or exclude), we need to match against the filenames
// relative to the project root.

/// Ensures that the configured hooks apply to at least one file in the repository.
pub(crate) async fn check_hooks_apply(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let store = STORE.as_ref()?;

    let relative_path = hook.project().relative_path();
    // Collect all files in the project
    let input = collect_files(hook.work_dir(), CollectOptions::all_files()).await?;
    // Prepend the project relative path to each input file
    let input: Vec<_> = input.into_iter().map(|f| relative_path.join(f)).collect();

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let path = relative_path.join(filename);
        let mut project = Project::from_config_file(path.into(), None)?;
        project.with_relative_path(relative_path.to_path_buf());

        let project_hooks = project.init_hooks(store, None).await?;
        let filter = FileFilter::for_project(input.iter(), &project);

        for project_hook in project_hooks {
            if project_hook.always_run || matches!(project_hook.language, Language::Fail) {
                continue;
            }

            let filenames = filter.for_hook(&project_hook);

            if filenames.is_empty() {
                code = 1;
                writeln!(
                    &mut output,
                    "{} does not apply to this repository",
                    project_hook.id
                )?;
            }
        }
    }

    Ok((code, output))
}

// Returns true if the exclude pattern matches any files matching the include pattern.
fn excludes_any(
    files: &[impl AsRef<Path>],
    include: Option<&Regex>,
    exclude: Option<&Regex>,
) -> bool {
    if exclude.is_none() {
        return true;
    }

    files.iter().any(|f| {
        let Some(f) = f.as_ref().to_str() else {
            return false; // Skip files that cannot be converted to a string
        };

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
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let relative_path = hook.project().relative_path();
    let input = collect_files(hook.work_dir(), CollectOptions::all_files()).await?;
    let input: Vec<_> = input.into_iter().map(|f| relative_path.join(f)).collect();

    let mut code = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let path = relative_path.join(filename);
        let mut project = Project::from_config_file(path.into(), None)?;
        project.with_relative_path(relative_path.to_path_buf());

        let config = project.config();
        if !excludes_any(&input, None, config.exclude.as_deref()) {
            code = 1;
            writeln!(
                &mut output,
                "The global exclude pattern `{}` does not match any files",
                config.exclude.as_deref().map_or("", |r| r.as_str())
            )?;
        }

        let filter = FileFilter::for_project(input.iter(), &project);

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
pub fn identity(_hook: &Hook, filenames: &[&Path]) -> (i32, Vec<u8>) {
    (
        0,
        filenames
            .iter()
            .map(|f| f.to_string_lossy())
            .join("\n")
            .into_bytes(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_excludes_any() {
        let files = vec![
            Path::new("file1.txt"),
            Path::new("file2.txt"),
            Path::new("file3.txt"),
        ];
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

        let files = vec![Path::new("html/file1.html"), Path::new("html/file2.html")];
        assert!(excludes_any(
            &files,
            None,
            Regex::new(r"^html/").ok().as_ref()
        ));
    }
}
