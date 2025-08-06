use std::path::Path;

use anyhow::Result;
use fancy_regex as regex;
use fancy_regex::Regex;
use itertools::{Either, Itertools};
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use rustc_hash::FxHashSet;
use tracing::{debug, error};

use constants::env_vars::EnvVars;

use crate::config::Stage;
use crate::fs::normalize_path;
use crate::hook::Hook;
use crate::identify::tags_from_path;
use crate::{git, warn_user};

/// Filter filenames by include/exclude patterns.
pub(crate) struct FilenameFilter {
    include: Option<Regex>,
    exclude: Option<Regex>,
}

impl FilenameFilter {
    pub(crate) fn new(
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<Self, Box<regex::Error>> {
        let include = include.map(Regex::new).transpose()?;
        let exclude = exclude.map(Regex::new).transpose()?;
        Ok(Self { include, exclude })
    }

    pub(crate) fn filter(&self, filename: impl AsRef<str>) -> bool {
        let filename = filename.as_ref();
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

    pub(crate) fn for_hook(hook: &Hook) -> Result<Self, Box<regex::Error>> {
        Self::new(hook.files.as_deref(), hook.exclude.as_deref())
    }
}

/// Filter files by tags.
struct FileTagFilter<'a> {
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

    fn filter(&self, file_types: &[&str]) -> bool {
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

    fn for_hook(hook: &'a Hook) -> Self {
        Self::new(&hook.types, &hook.types_or, &hook.exclude_types)
    }
}

pub(crate) struct FileFilter<'a> {
    filenames: Vec<&'a String>,
}

impl<'a> FileFilter<'a> {
    pub(crate) fn new(
        filenames: &'a [String],
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<Self, Box<regex::Error>> {
        let filter = FilenameFilter::new(include, exclude)?;

        let filenames = filenames
            .into_par_iter()
            .filter(|filename| filter.filter(filename))
            .collect::<Vec<_>>();

        Ok(Self { filenames })
    }

    pub(crate) fn len(&self) -> usize {
        self.filenames.len()
    }

    /// Filter filenames by tags for a specific hook.
    pub(crate) fn by_tag(&self, hook: &Hook) -> Vec<&String> {
        let filter = FileTagFilter::for_hook(hook);
        let filenames: Vec<_> = self
            .filenames
            .par_iter()
            .filter(|filename| {
                let path = Path::new(filename);
                match tags_from_path(path) {
                    Ok(tags) => filter.filter(&tags),
                    Err(err) => {
                        error!(filename, error = %err, "Failed to get tags");
                        false
                    }
                }
            })
            .copied()
            .collect();

        filenames
    }

    /// Filter filenames by file patterns and tags for a specific hook.
    pub(crate) fn for_hook(&self, hook: &Hook) -> Result<Vec<&String>, Box<regex::Error>> {
        let filter = FilenameFilter::for_hook(hook)?;
        let filenames = self
            .filenames
            .par_iter()
            .filter(|filename| filter.filter(filename));

        let filter = FileTagFilter::for_hook(hook);
        let filenames: Vec<_> = filenames
            .filter(|filename| {
                let path = Path::new(filename);
                match tags_from_path(path) {
                    Ok(tags) => filter.filter(&tags),
                    Err(err) => {
                        error!(filename, error = %err, "Failed to get tags");
                        false
                    }
                }
            })
            .copied()
            .collect();

        Ok(filenames)
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
pub(crate) async fn collect_files(opts: CollectOptions) -> Result<Vec<String>> {
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
    if EnvVars::is_set(EnvVars::PREFLIGIT_INTERNAL__SORT_FILENAMES) {
        filenames.sort_unstable();
    }

    for filename in &mut filenames {
        normalize_path(filename);
    }
    Ok(filenames)
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

        // TODO: It will be convenient to add a `--directory` flag to `prefligit run`,
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
