use std::path::{Path, PathBuf};

use anyhow::Result;
use fancy_regex as regex;
use fancy_regex::Regex;
use rayon::iter::{IntoParallelIterator, IntoParallelRefIterator, ParallelIterator};
use tracing::{debug, error};

use constants::env_vars::EnvVars;

use crate::config::Stage;
use crate::fs::normalize_path;
use crate::git;
use crate::hook::Hook;
use crate::identify::tags_from_path;

/// Filter filenames by include/exclude patterns.
pub struct FilenameFilter {
    include: Option<Regex>,
    exclude: Option<Regex>,
}

impl FilenameFilter {
    pub fn new(include: Option<&str>, exclude: Option<&str>) -> Result<Self, Box<regex::Error>> {
        let include = include.map(Regex::new).transpose()?;
        let exclude = exclude.map(Regex::new).transpose()?;
        Ok(Self { include, exclude })
    }

    pub fn filter(&self, filename: impl AsRef<str>) -> bool {
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

    pub fn from_hook(hook: &Hook) -> Result<Self, Box<regex::Error>> {
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

    fn from_hook(hook: &'a Hook) -> Self {
        Self::new(&hook.types, &hook.types_or, &hook.exclude_types)
    }
}

pub struct FileFilter<'a> {
    filenames: Vec<&'a String>,
}

impl<'a> FileFilter<'a> {
    pub fn new(
        filenames: &'a [String],
        include: Option<&str>,
        exclude: Option<&str>,
    ) -> Result<Self, Box<regex::Error>> {
        let filter = FilenameFilter::new(include, exclude)?;

        let filenames = filenames
            .into_par_iter()
            .filter(|filename| filter.filter(filename))
            .filter(|filename| {
                // TODO: does this check really necessary?
                // Ignore not existing files.
                std::fs::symlink_metadata(filename)
                    .map(|m| m.file_type().is_file())
                    .unwrap_or(false)
            })
            .collect::<Vec<_>>();

        Ok(Self { filenames })
    }

    pub fn len(&self) -> usize {
        self.filenames.len()
    }

    pub fn by_tag(&self, hook: &Hook) -> Vec<&String> {
        let filter = FileTagFilter::from_hook(hook);
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

    pub fn for_hook(&self, hook: &Hook) -> Result<Vec<&String>, Box<regex::Error>> {
        let filter = FilenameFilter::from_hook(hook)?;
        let filenames = self
            .filenames
            .par_iter()
            .filter(|filename| filter.filter(filename));

        let filter = FileTagFilter::from_hook(hook);
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
pub struct CollectOptions {
    pub hook_stage: Option<Stage>,
    pub from_ref: Option<String>,
    pub to_ref: Option<String>,
    pub all_files: bool,
    pub files: Vec<PathBuf>,
    pub commit_msg_filename: Option<PathBuf>,
}

impl CollectOptions {
    pub fn with_all_files(mut self, all_files: bool) -> Self {
        self.all_files = all_files;
        self
    }
}

/// Get all filenames to run hooks on.
#[allow(clippy::too_many_arguments)]
pub async fn collect_files(opts: CollectOptions) -> Result<Vec<String>> {
    let CollectOptions {
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        commit_msg_filename,
    } = opts;

    let mut filenames = collect_files_from_args(
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
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
    hook_stage: Option<Stage>,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<PathBuf>,
    commit_msg_filename: Option<PathBuf>,
) -> Result<Vec<String>> {
    if let Some(hook_stage) = hook_stage {
        if !hook_stage.operate_on_files() {
            return Ok(vec![]);
        }
        if hook_stage == Stage::PrepareCommitMsg || hook_stage == Stage::CommitMsg {
            return Ok(vec![
                commit_msg_filename
                    .expect("commit message filename is required")
                    .to_string_lossy()
                    .to_string(),
            ]);
        }
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

    if !files.is_empty() {
        let files: Vec<_> = files
            .into_iter()
            .map(|f| f.to_string_lossy().to_string())
            .collect();
        debug!("Files passed as arguments: {}", files.len());
        return Ok(files);
    }
    if all_files {
        let files = git::get_all_files().await?;
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
