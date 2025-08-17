use std::path::{Path, PathBuf};

use anyhow::Result;
use fancy_regex::Regex;
use itertools::{Either, Itertools};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};
use rustc_hash::FxHashSet;
use tracing::{debug, error};

use constants::env_vars::EnvVars;

use crate::config::Stage;
use crate::fs::normalize_path;
use crate::hook::Hook;
use crate::identify::tags_from_path;
use crate::workspace::Project;
use crate::{git, warn_user};

/// Filter filenames by include/exclude patterns.
pub(crate) struct FilenameFilter<'a> {
    include: Option<&'a Regex>,
    exclude: Option<&'a Regex>,
}

impl<'a> FilenameFilter<'a> {
    pub(crate) fn new(include: Option<&'a Regex>, exclude: Option<&'a Regex>) -> Self {
        Self { include, exclude }
    }

    pub(crate) fn filter(&self, filename: &Path) -> bool {
        let Some(filename) = filename.to_str() else {
            return false;
        };
        if let Some(re) = &self.include {
            if !re.is_match(filename).unwrap_or(false) {
                return false;
            }
        }
        if let Some(re) = &self.exclude {
            if re.is_match(filename).unwrap_or(false) {
                return false;
            }
        }
        true
    }

    pub(crate) fn for_hook(hook: &'a Hook) -> Self {
        Self::new(hook.files.as_deref(), hook.exclude.as_deref())
    }
}

/// Filter files by tags.
pub(crate) struct FileTagFilter<'a> {
    all: &'a [String],
    any: &'a [String],
    exclude: &'a [String],
}

impl<'a> FileTagFilter<'a> {
    fn new(types: &'a [String], types_or: &'a [String], exclude_types: &'a [String]) -> Self {
        Self {
            all: types,
            any: types_or,
            exclude: exclude_types,
        }
    }

    pub(crate) fn filter(&self, file_types: &[&str]) -> bool {
        if !self.all.is_empty() && !self.all.iter().all(|t| file_types.contains(&t.as_str())) {
            return false;
        }
        if !self.any.is_empty() && !self.any.iter().any(|t| file_types.contains(&t.as_str())) {
            return false;
        }
        if self
            .exclude
            .iter()
            .any(|t| file_types.contains(&t.as_str()))
        {
            return false;
        }
        true
    }

    pub(crate) fn for_hook(hook: &'a Hook) -> Self {
        Self::new(&hook.types, &hook.types_or, &hook.exclude_types)
    }
}

pub(crate) struct FileFilter<'a> {
    filenames: Vec<&'a Path>,
    filename_prefix: &'a Path,
}

impl<'a> FileFilter<'a> {
    pub(crate) fn for_project(filenames: &'a [&'a Path], project: &'a Project) -> Self {
        let filter = FilenameFilter::new(
            project.config().files.as_deref(),
            project.config().exclude.as_deref(),
        );

        // TODO: support orphaned project, which does not share files with its parent project.
        let filenames = filenames
            .par_iter()
            .filter(|filename| filter.filter(filename))
            // Collect files that are inside the hook project directory.
            .filter(|filename| filename.starts_with(project.relative_path()))
            .copied()
            .collect();

        Self {
            filenames,
            filename_prefix: project.relative_path(),
        }
    }

    pub(crate) fn filenames(&self) -> &[&Path] {
        &self.filenames
    }

    pub(crate) fn len(&self) -> usize {
        self.filenames.len()
    }

    /// Filter filenames by type tags for a specific hook.
    pub(crate) fn by_type(
        &self,
        types: &[String],
        types_or: &[String],
        exclude_types: &[String],
    ) -> Vec<&Path> {
        let filter = FileTagFilter::new(types, types_or, exclude_types);
        let filenames: Vec<_> = self
            .filenames
            .par_iter()
            .filter(|filename| match tags_from_path(filename) {
                Ok(tags) => filter.filter(&tags),
                Err(err) => {
                    error!(filename = ?filename.display(), error = %err, "Failed to get tags");
                    false
                }
            })
            .copied()
            .collect();

        filenames
    }

    /// Filter filenames by file patterns and tags for a specific hook.
    pub(crate) fn for_hook(&self, hook: &Hook) -> Vec<&Path> {
        // Filter by hook `files` and `exclude` patterns.
        let filter = FilenameFilter::for_hook(hook);
        let filenames = self
            .filenames
            .par_iter()
            .filter(|filename| filter.filter(filename));

        // Filter by hook `types`, `types_or` and `exclude_types`.
        let filter = FileTagFilter::for_hook(hook);
        let filenames = filenames.filter(|filename| match tags_from_path(filename) {
            Ok(tags) => filter.filter(&tags),
            Err(err) => {
                error!(filename = ?filename.display(), error = %err, "Failed to get tags");
                false
            }
        });

        // Strip the prefix to get relative paths.
        let filenames: Vec<_> = filenames
            .map(|p| {
                p.strip_prefix(self.filename_prefix)
                    .expect("Failed to strip prefix")
            })
            .collect();

        filenames
    }
}

#[derive(Default)]
pub(crate) struct CollectOptions {
    pub(crate) hook_stage: Stage,
    pub(crate) from_ref: Option<String>,
    pub(crate) to_ref: Option<String>,
    pub(crate) all_files: bool,
    pub(crate) files: Vec<String>,
    pub(crate) directories: Vec<String>,
    pub(crate) commit_msg_filename: Option<String>,
}

impl CollectOptions {
    pub(crate) fn with_all_files(mut self, all_files: bool) -> Self {
        self.all_files = all_files;
        self
    }
}

/// Get all filenames to run hooks on.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn collect_files(opts: CollectOptions) -> Result<Vec<PathBuf>> {
    let CollectOptions {
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        directories,
        commit_msg_filename,
    } = opts;

    let mut filenames = collect_files_from_args(
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        directories,
        commit_msg_filename,
    )
    .await?;

    // Sort filenames if in tests to make the order consistent.
    if EnvVars::is_set(EnvVars::PREK_INTERNAL__SORT_FILENAMES) {
        filenames.sort_unstable();
    }

    for filename in &mut filenames {
        normalize_path(filename);
    }

    Ok(filenames.into_iter().map(PathBuf::from).collect())
}

#[allow(clippy::too_many_arguments)]
async fn collect_files_from_args(
    hook_stage: Stage,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    mut files: Vec<String>,
    mut directories: Vec<String>,
    commit_msg_filename: Option<String>,
) -> Result<Vec<String>> {
    if !hook_stage.operate_on_files() {
        return Ok(vec![]);
    }
    // TODO: adjust relative path to based on the workspace root
    if hook_stage == Stage::PrepareCommitMsg || hook_stage == Stage::CommitMsg {
        return Ok(vec![
            commit_msg_filename.expect("commit message filename is required"),
        ]);
    }

    if let (Some(from_ref), Some(to_ref)) = (from_ref, to_ref) {
        let files = git::get_changed_files(&from_ref, &to_ref).await?;
        debug!(
            "Files changed between {} and {}: {}",
            from_ref,
            to_ref,
            files.len()
        );
        return Ok(files);
    }

    if !files.is_empty() || !directories.is_empty() {
        // By default, `pre-commit` add `types: [file]` for all hooks,
        // so `pre-commit` will ignore user provided directories.
        // We do the same here for compatibility.
        // For `types: [directory]`, `pre-commit` passes the directory names to the hook directly.

        // Fun fact: if a hook specified `types: [directory]`, it won't run in `--all-files` mode.

        // TODO: It will be convenient to add a `--directory` flag to `prek run`,
        // we expand the directories to files and pass them to the hook.
        // See: https://github.com/pre-commit/pre-commit/issues/1173

        // Normalize paths for HashSet to work correctly.
        for filename in &mut files {
            normalize_path(filename);
        }
        for dir in &mut directories {
            normalize_path(dir);
        }

        let (mut exists, non_exists): (FxHashSet<_>, Vec<_>) =
            files.into_iter().partition_map(|filename| {
                if Path::new(&filename).exists() {
                    Either::Left(filename)
                } else {
                    Either::Right(filename)
                }
            });
        if !non_exists.is_empty() {
            if non_exists.len() == 1 {
                warn_user!(
                    "This file does not exist, it will be ignored: `{}`",
                    non_exists[0]
                );
            } else if non_exists.len() == 2 {
                warn_user!(
                    "These files do not exist, they will be ignored: `{}`",
                    non_exists.join(", ")
                );
            }
        }

        for dir in directories {
            let dir_files = git::git_ls_files(Some(Path::new(&dir))).await?;
            for file in dir_files {
                exists.insert(file);
            }
        }

        debug!("Files passed as arguments: {}", exists.len());
        return Ok(exists.into_iter().collect());
    }

    if all_files {
        let files = git::git_ls_files(None).await?;
        debug!("All files in the repo: {}", files.len());
        return Ok(files);
    }

    if git::is_in_merge_conflict().await? {
        let files = git::get_conflicted_files().await?;
        debug!("Conflicted files: {}", files.len());
        return Ok(files);
    }

    let files = git::get_staged_files().await?;
    debug!("Staged files: {}", files.len());

    Ok(files)
}
