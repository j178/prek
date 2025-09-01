use std::path::{Path, PathBuf};

use anyhow::Result;
use fancy_regex::Regex;
use itertools::{Either, Itertools};
use rayon::iter::{IntoParallelRefIterator, ParallelBridge, ParallelIterator};
use rustc_hash::FxHashSet;
use tracing::{debug, error};

use constants::env_vars::EnvVars;

use crate::config::Stage;
use crate::fs::normalize_path;
use crate::git::GIT_ROOT;
use crate::hook::Hook;
use crate::identify::tags_from_path;
use crate::workspace::Project;
use crate::{fs, git, warn_user};

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
    pub(crate) fn for_project<I>(filenames: I, project: &'a Project) -> Self
    where
        I: Iterator<Item = &'a PathBuf> + Send,
    {
        let filter = FilenameFilter::new(
            project.config().files.as_deref(),
            project.config().exclude.as_deref(),
        );

        // TODO: support orphaned project, which does not share files with its parent project.
        let mut filenames = filenames
            .enumerate()
            .par_bridge()
            .map(|(i, p)| (i, p.as_path()))
            .filter(|(_, filename)| filter.filter(filename))
            // Collect files that are inside the hook project directory.
            .filter(|(_, filename)| filename.starts_with(project.relative_path()))
            .collect::<Vec<_>>();

        // Keep filename order consistent
        filenames.sort_by_key(|&(i, _)| i);

        Self {
            filenames: filenames.into_iter().map(|(_, p)| p).collect(),
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
    pub(crate) fn all_files() -> Self {
        Self {
            all_files: true,
            ..Default::default()
        }
    }
}

/// Get all filenames to run hooks on.
/// Returns a list of file paths relative to the workspace root.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn collect_files(root: &Path, opts: CollectOptions) -> Result<Vec<PathBuf>> {
    let CollectOptions {
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        directories,
        commit_msg_filename,
    } = opts;

    let git_root = GIT_ROOT.as_ref()?;

    // The workspace root relative to the git root.
    let relative_root = match root.strip_prefix(git_root)? {
        p if p.as_os_str().is_empty() => None,
        p => Some(p),
    };

    let filenames = collect_files_from_args(
        git_root,
        relative_root,
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        directories,
        commit_msg_filename,
    )
    .await?;

    // Convert filenames to be relative to the workspace root.
    let mut filenames = filenames
        .into_iter()
        .filter_map(|filename| {
            // Only keep files under the workspace root.
            if let Some(relative_root) = relative_root {
                filename
                    .strip_prefix(relative_root)
                    .map(|p| normalize_path(p.to_path_buf()))
                    .ok()
            } else {
                Some(normalize_path(filename))
            }
        })
        .collect::<Vec<_>>();

    // Sort filenames if in tests to make the order consistent.
    if EnvVars::is_set(EnvVars::PREK_INTERNAL__SORT_FILENAMES) {
        filenames.sort_unstable();
    }

    Ok(filenames)
}

fn adjust_relative_path(path: &str, new_cwd: &Path) -> Result<PathBuf, std::io::Error> {
    fs::relative_to(std::path::absolute(path)?, new_cwd)
}

/// Collect files to run hooks on.
/// Returns a list of file paths relative to the workspace root.
#[allow(clippy::too_many_arguments)]
async fn collect_files_from_args(
    git_root: &Path,
    relative_root: Option<&Path>,
    hook_stage: Stage,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<String>,
    directories: Vec<String>,
    commit_msg_filename: Option<String>,
) -> Result<Vec<PathBuf>> {
    if !hook_stage.operate_on_files() {
        return Ok(vec![]);
    }

    if hook_stage == Stage::PrepareCommitMsg || hook_stage == Stage::CommitMsg {
        let path = commit_msg_filename.expect("commit_msg_filename should be set");
        let path = adjust_relative_path(&path, git_root)?;
        return Ok(vec![path]);
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

        let (exists, non_exists): (FxHashSet<_>, Vec<_>) =
            files.into_iter().partition_map(|filename| {
                if std::fs::exists(&filename).unwrap_or(false) {
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
            } else {
                warn_user!(
                    "These files do not exist, they will be ignored: `{}`",
                    non_exists.join(", ")
                );
            }
        }

        let mut exists = exists
            .into_iter()
            .map(|filename| adjust_relative_path(&filename, git_root).map(normalize_path))
            .collect::<Result<FxHashSet<_>, _>>()?;

        for dir in directories {
            let dir = adjust_relative_path(&dir, git_root)?;
            let dir_files = git::ls_files(git_root, Some(&dir)).await?;
            for file in dir_files {
                let file = normalize_path(file);
                exists.insert(file);
            }
        }

        debug!("Files passed as arguments: {}", exists.len());
        return Ok(exists.into_iter().collect());
    }

    if all_files {
        let files = git::ls_files(git_root, relative_root).await?;
        debug!("All files in the workspace: {}", files.len());
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
