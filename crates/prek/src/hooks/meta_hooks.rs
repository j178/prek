use std::io::Write;
use std::ops::ControlFlow;
use std::path::Path;
use std::str::FromStr;

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::CONFIG_FILENAMES;

use crate::cli::reporter::HookRunReporter;
use crate::cli::run::{
    CollectOptions, FileTagCache, FileTagFilter, HookFileFilter, ProjectFiles, collect_run_input,
};
use crate::config::{self, FilePattern, HookOptions, Language, MetaHook};
use crate::hook::Hook;
use crate::store::Store;
use crate::workspace::Project;

// For builtin hooks (meta hooks and builtin pre-commit-hooks), they are not run
// in the project root like other hooks. Instead, they run in the workspace root.
// But the input filenames are all relative to the project root. So when accessing these files,
// we need to adjust the paths by prepending the project relative path.
// When matching files (files or exclude), we need to match against the filenames
// relative to the project root.

#[derive(Debug, Copy, Clone, PartialEq, Eq, strum::AsRefStr, strum::Display, strum::EnumString)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(rename_all = "kebab-case"))]
#[strum(serialize_all = "kebab-case")]
pub(crate) enum MetaHooks {
    CheckHooksApply,
    CheckUselessExcludes,
    Identity,
}

impl MetaHooks {
    pub(crate) async fn run(
        self,
        store: &Store,
        hook: &Hook,
        filenames: &[&Path],
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());
        let result = match self {
            Self::CheckHooksApply => check_hooks_apply(store, hook, filenames).await,
            Self::CheckUselessExcludes => check_useless_excludes(hook, filenames).await,
            Self::Identity => Ok(identity(hook, filenames)),
        };
        reporter.on_run_complete(progress, matches!(&result, Ok((0, _))));
        result
    }
}

impl MetaHook {
    pub(crate) fn from_id(id: &str) -> Result<Self, ()> {
        let hook_id = MetaHooks::from_str(id).map_err(|_| ())?;
        let config_file_glob =
            FilePattern::glob(CONFIG_FILENAMES.iter().map(ToString::to_string).collect()).unwrap();

        Ok(match hook_id {
            MetaHooks::CheckHooksApply => MetaHook {
                id: "check-hooks-apply".to_string(),
                name: "Check hooks apply".to_string(),
                priority: None,
                options: HookOptions {
                    files: Some(config_file_glob.clone()),
                    ..Default::default()
                },
            },
            MetaHooks::CheckUselessExcludes => MetaHook {
                id: "check-useless-excludes".to_string(),
                name: "Check useless excludes".to_string(),
                priority: None,
                options: HookOptions {
                    files: Some(config_file_glob),
                    ..Default::default()
                },
            },
            MetaHooks::Identity => MetaHook {
                id: "identity".to_string(),
                name: "identity".to_string(),
                priority: None,
                options: HookOptions {
                    verbose: Some(true),
                    ..Default::default()
                },
            },
        })
    }
}

/// Ensures that the configured hooks apply to at least one file in the repository.
pub(crate) async fn check_hooks_apply(
    store: &Store,
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let projects = load_meta_projects(hook, filenames)?;
    if projects.is_empty() {
        return Ok((0, Vec::new()));
    }

    let relative_path = hook.project().relative_path();
    // Collect all files in the project
    let input = collect_run_input(hook.work_dir(), CollectOptions::all_files())
        .await?
        .into_files();
    // Prepend the project relative path to each input file
    let input: Vec<_> = input.into_iter().map(|f| relative_path.join(f)).collect();

    let mut code = 0;
    let mut output = Vec::new();
    let mut tag_cache = FileTagCache::default();

    for project in projects {
        let project_hooks = project
            .init_hooks(store, None)
            .await
            .context("Failed to init hooks")?;
        let hooks = project_hooks
            .iter()
            .filter(|hook| !hook.always_run && hook.language != Language::Fail)
            .collect::<Vec<_>>();
        if hooks.is_empty() {
            continue;
        }

        let filters = hooks
            .iter()
            .map(|hook| HookFileFilter::new(hook))
            .collect::<Vec<_>>();
        let mut matches = vec![false; hooks.len()];
        let mut remaining = matches.len();

        ProjectFiles::visit_for_project(input.iter(), hooks[0].project(), None, None, |file| {
            let tags = file.tags(&mut tag_cache);
            for (matched, filter) in matches.iter_mut().zip(&filters) {
                if *matched {
                    continue;
                }
                if filter.matches_filename(file.hook_path()) && filter.matches_tags(tags) {
                    *matched = true;
                    remaining -= 1;
                }
            }

            if remaining == 0 {
                return ControlFlow::Break(());
            }

            ControlFlow::Continue(())
        });

        for (project_hook, matches) in hooks.iter().zip(matches) {
            if !matches {
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

fn load_meta_projects(hook: &Hook, filenames: &[&Path]) -> Result<Vec<Project>> {
    let relative_path = hook.project().relative_path();
    filenames
        .iter()
        .map(|filename| {
            let path = relative_path.join(filename);
            let mut project = Project::from_config_file(path.into(), None)?;
            project.with_relative_path(relative_path.to_path_buf());
            Ok(project)
        })
        .collect()
}

fn extend_hook_options<'a>(
    repo: &'a config::Repo,
    hook_options: &mut Vec<(&'a String, &'a HookOptions)>,
) {
    match repo {
        config::Repo::Remote(repo) => {
            hook_options.extend(repo.hooks.iter().map(|hook| (&hook.id, &hook.options)));
        }
        config::Repo::Local(repo) => {
            hook_options.extend(repo.hooks.iter().map(|hook| (&hook.id, &hook.options)));
        }
        config::Repo::Meta(repo) => {
            hook_options.extend(repo.hooks.iter().map(|hook| (&hook.id, &hook.options)));
        }
        config::Repo::Builtin(repo) => {
            hook_options.extend(repo.hooks.iter().map(|hook| (&hook.id, &hook.options)));
        }
    }
}

fn matches_patterns(
    filename: &Path,
    include: Option<&FilePattern>,
    exclude: Option<&FilePattern>,
) -> bool {
    if let Some(pattern) = include {
        if !pattern.is_match(filename) {
            return false;
        }
    }
    if let Some(pattern) = exclude {
        if !pattern.is_match(filename) {
            return false;
        }
    }
    true
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

    files
        .iter()
        .any(|f| matches_patterns(f.as_ref(), include, exclude))
}

/// Ensures that exclude directives apply to any file in the repository.
pub(crate) async fn check_useless_excludes(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let projects = load_meta_projects(hook, filenames)?;
    if projects.is_empty() {
        return Ok((0, Vec::new()));
    }

    let relative_path = hook.project().relative_path();
    // `collect_run_input` returns paths relative to the hook's project root.
    // The meta hook itself runs from the workspace root, so we build both:
    // - `input_project`: for matching `files`/`exclude` patterns (project-relative)
    // - `input_workspace`: for project ownership and type matching (workspace-relative)
    let input_project = collect_run_input(hook.work_dir(), CollectOptions::all_files())
        .await?
        .into_files();
    let input_workspace: Vec<_> = input_project
        .iter()
        .map(|f| relative_path.join(f))
        .collect();

    let mut code = 0;
    let mut output = Vec::new();
    let mut tag_cache = FileTagCache::default();

    for project in projects {
        let config = project.config();
        if !excludes_any(&input_project, None, config.exclude.as_ref()) {
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

        let mut hook_options = Vec::new();
        for repo in &config.repos {
            extend_hook_options(repo, &mut hook_options);
        }
        if hook_options.iter().all(|(_, opts)| opts.exclude.is_none()) {
            continue;
        }

        let tag_filters = hook_options
            .iter()
            .map(|(_, opts)| {
                FileTagFilter::new(
                    opts.types.as_ref(),
                    opts.types_or.as_ref(),
                    opts.exclude_types.as_ref(),
                )
            })
            .collect::<Vec<_>>();
        let mut exclude_matches = hook_options
            .iter()
            .map(|(_, opts)| opts.exclude.is_none())
            .collect::<Vec<_>>();
        let mut remaining = exclude_matches.iter().filter(|matched| !**matched).count();

        ProjectFiles::visit_for_project(input_workspace.iter(), &project, None, None, |file| {
            let tags = file.tags(&mut tag_cache);
            for ((matched, (_, opts)), tag_filter) in exclude_matches
                .iter_mut()
                .zip(&hook_options)
                .zip(&tag_filters)
            {
                if *matched || tags.is_none_or(|tags| !tag_filter.matches(tags)) {
                    continue;
                }

                if matches_patterns(file.hook_path(), opts.files.as_ref(), opts.exclude.as_ref()) {
                    *matched = true;
                    remaining -= 1;
                }
            }

            if remaining == 0 {
                return ControlFlow::Break(());
            }

            ControlFlow::Continue(())
        });

        for ((hook_id, opts), exclude_matches) in hook_options.iter().zip(exclude_matches) {
            if !exclude_matches {
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
    use prek_consts::{PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML, PREK_TOML};

    fn regex_pattern(pattern: &str) -> FilePattern {
        FilePattern::regex(pattern).unwrap()
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
        let apply_files = apply.options.files.as_ref().expect("files should be set");
        assert!(apply_files.is_match(Path::new(PRE_COMMIT_CONFIG_YAML)));
        assert!(apply_files.is_match(Path::new(PRE_COMMIT_CONFIG_YML)));
        assert!(apply_files.is_match(Path::new(PREK_TOML)));

        let useless = MetaHook::from_id("check-useless-excludes").expect("known meta hook");
        let useless_files = useless.options.files.as_ref().expect("files should be set");
        assert!(useless_files.is_match(Path::new(PRE_COMMIT_CONFIG_YAML)));
        assert!(useless_files.is_match(Path::new(PRE_COMMIT_CONFIG_YML)));
        assert!(useless_files.is_match(Path::new(PREK_TOML)));

        let identity = MetaHook::from_id("identity").expect("known meta hook");
        assert!(identity.options.files.is_none());
        assert_eq!(identity.options.verbose, Some(true));
    }
}
