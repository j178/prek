use std::io::Write;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use itertools::Itertools;

use crate::cli::run::{CollectOptions, FileFilter, collect_files};
use crate::config::{
    self, CONFIG_FILE_REGEX, FilePattern, HookOptions, Language, ManifestHook, MetaHook,
};
use crate::hook::Hook;
use crate::store::Store;
use crate::workspace::Project;

// For builtin hooks (meta hooks and builtin pre-commit-hooks), they are not run
// in the project root like other hooks. Instead, they run in the workspace root.
// But the input filenames are all relative to the project root. So when accessing these files,
// we need to adjust the paths by prepending the project relative path.
// When matching files (files or exclude), we need to match against the filenames
// relative to the project root.

#[derive(Debug, Copy, Clone)]
pub(crate) enum MetaHooks {
    CheckHooksApply,
    CheckUselessExcludes,
    Identity,
}

impl FromStr for MetaHooks {
    type Err = ();

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s {
            "check-hooks-apply" => Ok(Self::CheckHooksApply),
            "check-useless-excludes" => Ok(Self::CheckUselessExcludes),
            "identity" => Ok(Self::Identity),
            _ => Err(()),
        }
    }
}

impl MetaHooks {
    pub(crate) async fn run(
        self,
        store: &Store,
        hook: &Hook,
        filenames: &[&Path],
    ) -> Result<(i32, Vec<u8>)> {
        match self {
            Self::CheckHooksApply => check_hooks_apply(store, hook, filenames).await,
            Self::CheckUselessExcludes => check_useless_excludes(hook, filenames).await,
            Self::Identity => Ok(identity(hook, filenames)),
        }
    }
}

impl MetaHook {
    pub(crate) fn from_id(id: &str) -> Result<Self, ()> {
        let hook_id = MetaHooks::from_str(id)?;
        let config_file_regex = CONFIG_FILE_REGEX.clone();

        let hook = match hook_id {
            MetaHooks::CheckHooksApply => ManifestHook {
                id: "check-hooks-apply".to_string(),
                name: "Check hooks apply".to_string(),
                language: Language::System,
                entry: String::new(),
                options: HookOptions {
                    files: Some(FilePattern::from(config_file_regex)),
                    ..Default::default()
                },
            },
            MetaHooks::CheckUselessExcludes => ManifestHook {
                id: "check-useless-excludes".to_string(),
                name: "Check useless excludes".to_string(),
                language: Language::System,
                entry: String::new(),
                options: HookOptions {
                    files: Some(FilePattern::from(config_file_regex)),
                    ..Default::default()
                },
            },
            MetaHooks::Identity => ManifestHook {
                id: "identity".to_string(),
                name: "identity".to_string(),
                language: Language::System,
                entry: String::new(),
                options: HookOptions {
                    verbose: Some(true),
                    ..Default::default()
                },
            },
        };

        Ok(MetaHook(hook))
    }
}

/// Ensures that the configured hooks apply to at least one file in the repository.
pub(crate) async fn check_hooks_apply(
    store: &Store,
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
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

        let project_hooks = project
            .init_hooks(store, None)
            .await
            .context("Failed to init hooks")?;
        let filter = FileFilter::for_project(input.iter(), &project, None);

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
    include: Option<&FilePattern>,
    exclude: Option<&FilePattern>,
) -> bool {
    if exclude.is_none() {
        return true;
    }

    files.iter().any(|f| {
        let Some(f) = f.as_ref().to_str() else {
            return false; // Skip files that cannot be converted to a string
        };

        if let Some(pattern) = &include {
            if !pattern.is_match(f) {
                return false;
            }
        }
        if let Some(pattern) = &exclude {
            if !pattern.is_match(f) {
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
        if !excludes_any(&input, None, config.exclude.as_ref()) {
            code = 1;
            let display = config
                .exclude
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default();
            writeln!(
                &mut output,
                "The global exclude pattern `{display}` does not match any files"
            )?;
        }

        let filter = FileFilter::for_project(input.iter(), &project, None);

        for repo in &config.repos {
            let hooks_iter: Box<dyn Iterator<Item = (&String, &HookOptions)>> = match repo {
                config::Repo::Remote(r) => Box::new(r.hooks.iter().map(|h| (&h.id, &h.options))),
                config::Repo::Local(r) => Box::new(r.hooks.iter().map(|h| (&h.id, &h.options))),
                config::Repo::Meta(r) => Box::new(r.hooks.iter().map(|h| (&h.0.id, &h.0.options))),
                config::Repo::Builtin(r) => {
                    Box::new(r.hooks.iter().map(|h| (&h.0.id, &h.0.options)))
                }
            };

            for (hook_id, opts) in hooks_iter {
                let filtered_files = filter.by_type(
                    opts.types.as_deref().unwrap_or(&[]),
                    opts.types_or.as_deref().unwrap_or(&[]),
                    opts.exclude_types.as_deref().unwrap_or(&[]),
                );

                if !excludes_any(&filtered_files, opts.files.as_ref(), opts.exclude.as_ref()) {
                    code = 1;
                    let display = opts
                        .exclude
                        .as_ref()
                        .map(ToString::to_string)
                        .unwrap_or_default();
                    writeln!(
                        &mut output,
                        "The exclude pattern `{display}` for `{hook_id}` does not match any files"
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
    use prek_consts::{ALT_CONFIG_FILE, CONFIG_FILE};

    fn regex_pattern(pattern: &str) -> FilePattern {
        let regex = fancy_regex::Regex::new(pattern).unwrap();
        FilePattern::from(regex)
    }

    #[test]
    fn test_excludes_any() {
        let files = vec![
            Path::new("file1.txt"),
            Path::new("file2.txt"),
            Path::new("file3.txt"),
        ];
        let include = regex_pattern(r"file.*");
        let exclude = regex_pattern(r"file2\.txt");
        assert!(excludes_any(&files, Some(&include), Some(&exclude)));

        let include = regex_pattern(r"file.*");
        let exclude = regex_pattern(r"file4\.txt");
        assert!(!excludes_any(&files, Some(&include), Some(&exclude)));
        assert!(excludes_any(&files, None, None));

        let files = vec![Path::new("html/file1.html"), Path::new("html/file2.html")];
        let exclude = regex_pattern(r"^html/");
        assert!(excludes_any(&files, None, Some(&exclude)));
    }

    #[test]
    fn meta_hook_patterns_cover_config_files() {
        let apply = MetaHook::from_id("check-hooks-apply").expect("known meta hook");
        let apply_files = apply.0.options.files.as_ref().expect("files should be set");
        assert!(apply_files.is_match(CONFIG_FILE));
        assert!(apply_files.is_match(ALT_CONFIG_FILE));

        let useless = MetaHook::from_id("check-useless-excludes").expect("known meta hook");
        let useless_files = useless
            .0
            .options
            .files
            .as_ref()
            .expect("files should be set");
        assert!(useless_files.is_match(CONFIG_FILE));
        assert!(useless_files.is_match(ALT_CONFIG_FILE));

        let identity = MetaHook::from_id("identity").expect("known meta hook");
        assert!(identity.0.options.files.is_none());
        assert_eq!(identity.0.options.verbose, Some(true));
    }
}
