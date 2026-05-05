use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use itertools::{Either, Itertools};
use path_clean::PathClean;
use prek_consts::env_vars::EnvVars;
use prek_identify::{TagSet, tags_from_path};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::{debug, error, instrument};

use crate::config::{FilePattern, Stage};
use crate::git::GIT_ROOT;
use crate::hook::Hook;
use crate::workspace::Project;
use crate::{fs, git, warn_user};

/// Filter filenames by include/exclude patterns.
pub(crate) struct FilenameFilter<'a> {
    include: Option<&'a FilePattern>,
    exclude: Option<&'a FilePattern>,
}

impl<'a> FilenameFilter<'a> {
    pub(crate) fn new(include: Option<&'a FilePattern>, exclude: Option<&'a FilePattern>) -> Self {
        Self { include, exclude }
    }

    pub(crate) fn matches(&self, filename: &Path) -> bool {
        if let Some(pattern) = &self.include {
            if !pattern.is_match(filename) {
                return false;
            }
        }
        if let Some(pattern) = &self.exclude {
            if pattern.is_match(filename) {
                return false;
            }
        }
        true
    }
}

/// Filter files by tags.
pub(crate) struct FileTagFilter<'a> {
    all: Option<&'a TagSet>,
    any: Option<&'a TagSet>,
    exclude: Option<&'a TagSet>,
}

impl<'a> FileTagFilter<'a> {
    fn new(
        types: Option<&'a TagSet>,
        types_or: Option<&'a TagSet>,
        exclude_types: Option<&'a TagSet>,
    ) -> Self {
        Self {
            all: types,
            any: types_or,
            exclude: exclude_types,
        }
    }

    pub(crate) fn matches(&self, file_types: &TagSet) -> bool {
        if self.all.is_some_and(|s| !s.is_subset(file_types)) {
            return false;
        }
        if self
            .any
            .is_some_and(|s| !s.is_empty() && s.is_disjoint(file_types))
        {
            return false;
        }
        if self.exclude.is_some_and(|s| !s.is_disjoint(file_types)) {
            return false;
        }
        true
    }
}

pub(crate) struct HookFileFilter<'a> {
    filename: FilenameFilter<'a>,
    tags: FileTagFilter<'a>,
}

impl<'a> HookFileFilter<'a> {
    pub(crate) fn new(hook: &'a Hook) -> Self {
        Self {
            filename: FilenameFilter::new(hook.files.as_ref(), hook.exclude.as_ref()),
            tags: FileTagFilter::new(
                Some(&hook.types),
                Some(&hook.types_or),
                Some(&hook.exclude_types),
            ),
        }
    }

    pub(crate) fn matches_filename(&self, filename: &Path) -> bool {
        self.filename.matches(filename)
    }

    pub(crate) fn matches_tags(&self, tags: Option<&TagSet>) -> bool {
        tags.is_some_and(|tags| self.tags.matches(tags))
    }

    fn matches_project_file(&self, file: &ProjectFile<'_>, tag_cache: &mut FileTagCache) -> bool {
        self.matches_filename(file.hook_path) && self.matches_tags(file.tags(tag_cache))
    }
}

struct ProjectFile<'a> {
    workspace_path: &'a Path,
    hook_path: &'a Path,
}

impl<'a> ProjectFile<'a> {
    fn new(workspace_path: &'a Path, hook_path: &'a Path) -> Self {
        Self {
            workspace_path,
            hook_path,
        }
    }

    fn tags<'cache>(&self, tag_cache: &'cache mut FileTagCache) -> Option<&'cache TagSet> {
        tag_cache.tags(self.workspace_path)
    }
}

#[derive(Default)]
pub(crate) struct FileTagCache {
    tags_by_path: FxHashMap<PathBuf, Option<TagSet>>,
}

impl FileTagCache {
    pub(crate) fn tags(&mut self, path: &Path) -> Option<&TagSet> {
        if !self.tags_by_path.contains_key(path) {
            let tags = match tags_from_path(path) {
                Ok(tags) => Some(tags),
                Err(err) => {
                    error!(filename = ?path.display(), error = %err, "Failed to get tags");
                    None
                }
            };
            self.tags_by_path.insert(path.to_path_buf(), tags);
        }
        self.tags_by_path.get(path).and_then(Option::as_ref)
    }
}

pub(crate) struct ProjectFiles<'a> {
    files: Vec<ProjectFile<'a>>,
}

impl<'a> ProjectFiles<'a> {
    /// Create project-owned files after applying the project's relative path and include/exclude patterns.
    /// `filenames` are paths relative to the workspace root.
    pub(crate) fn for_project<I>(
        filenames: I,
        project: &Project,
        mut consumed_files: Option<&mut FxHashSet<&'a Path>>,
    ) -> Self
    where
        I: Iterator<Item = &'a PathBuf> + Send,
    {
        let filename_filter = FilenameFilter::new(
            project.config().files.as_ref(),
            project.config().exclude.as_ref(),
        );
        let relative_path = project.relative_path();
        let orphan = project.config().orphan.unwrap_or(false);

        // The order of below filters matters.
        // If this is an orphan project, we must mark all files in its directory as consumed
        // *before* applying the project's include/exclude patterns. This ensures that even
        // files excluded by this project are still considered "owned" by it and hidden
        // from parent projects.
        let files = filenames
            .map(PathBuf::as_path)
            // Collect files that are inside the hook project directory.
            .filter(|filename| filename.starts_with(relative_path))
            // Skip files that have already been consumed by subprojects.
            .filter(|filename| {
                if let Some(consumed_files) = consumed_files.as_mut() {
                    if orphan {
                        return consumed_files.insert(filename);
                    }
                    !consumed_files.contains(filename)
                } else {
                    true
                }
            })
            // Strip the project-relative prefix before applying project-level include/exclude patterns.
            .filter_map(|filename| {
                let relative = filename
                    .strip_prefix(relative_path)
                    .expect("Filename should start with project relative path");
                if filename_filter.matches(relative) {
                    Some(ProjectFile::new(filename, relative))
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();

        Self { files }
    }

    pub(crate) fn len(&self) -> usize {
        self.files.len()
    }

    /// Filter filenames by type tags for a specific hook.
    pub(crate) fn by_type(
        &self,
        types: Option<&TagSet>,
        types_or: Option<&TagSet>,
        exclude_types: Option<&TagSet>,
        tag_cache: &mut FileTagCache,
    ) -> Vec<&Path> {
        let tag_filter = FileTagFilter::new(types, types_or, exclude_types);
        let mut filenames = Vec::new();
        for file in &self.files {
            if let Some(tags) = file.tags(tag_cache) {
                if tag_filter.matches(tags) {
                    filenames.push(file.workspace_path);
                }
            }
        }
        filenames
    }

    /// Filter filenames by file patterns and tags for a specific hook.
    #[instrument(level = "trace", skip_all, fields(hook = ?hook.id))]
    pub(crate) fn for_hook(&self, hook: &Hook, tag_cache: &mut FileTagCache) -> Vec<&Path> {
        let hook_filter = HookFileFilter::new(hook);
        let mut filenames = Vec::new();
        for file in &self.files {
            if hook_filter.matches_project_file(file, tag_cache) {
                filenames.push(file.hook_path);
            }
        }
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

pub(crate) enum RunInput {
    /// File paths relative to the workspace root.
    Files(Vec<PathBuf>),
    /// Absolute path to the Git message file passed by `commit-msg` and `prepare-commit-msg`.
    MessageFile(PathBuf),
}

/// Get hook input for the selected stage.
pub(crate) async fn collect_run_input(root: &Path, opts: CollectOptions) -> Result<RunInput> {
    let CollectOptions {
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        directories,
        commit_msg_filename,
    } = opts;

    if matches!(hook_stage, Stage::PrepareCommitMsg | Stage::CommitMsg) {
        let path = commit_msg_filename.expect("commit_msg_filename should be set");
        return Ok(RunInput::MessageFile(GIT_ROOT.as_ref()?.join(path)));
    }

    collect_workspace_files(
        root,
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        directories,
    )
    .await
    .map(RunInput::Files)
}

/// Get workspace filenames to run hooks on.
/// Returns a list of file paths relative to the workspace root.
pub(crate) async fn collect_files(root: &Path, opts: CollectOptions) -> Result<Vec<PathBuf>> {
    match collect_run_input(root, opts).await? {
        RunInput::Files(files) => Ok(files),
        // This compatibility API can only return workspace-relative files.
        // Git message files are hook arguments, not workspace files, and are
        // handled through `RunInput` by the main runner.
        RunInput::MessageFile(_) => Ok(vec![]),
    }
}

#[allow(clippy::too_many_arguments)]
#[instrument(level = "trace", skip_all)]
async fn collect_workspace_files(
    root: &Path,
    hook_stage: Stage,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<String>,
    directories: Vec<String>,
) -> Result<Vec<PathBuf>> {
    let git_root = GIT_ROOT.as_ref()?;

    // The workspace root relative to the git root.
    let relative_root = root.strip_prefix(git_root).with_context(|| {
        format!(
            "Workspace root `{}` is not under git root `{}`",
            root.display(),
            git_root.display()
        )
    })?;

    let filenames = collect_files_from_args(
        git_root,
        root,
        hook_stage,
        from_ref,
        to_ref,
        all_files,
        files,
        directories,
    )
    .await?;

    // Convert filenames to be relative to the workspace root.
    let mut filenames = filenames
        .into_iter()
        .filter_map(|filename| {
            // Only keep files under the workspace root.
            filename
                .strip_prefix(relative_root)
                .map(|p| fs::normalize_path(p.to_path_buf()))
                .ok()
        })
        .collect::<Vec<_>>();

    // Sort filenames if in tests to make the order consistent.
    if EnvVars::is_set(EnvVars::PREK_INTERNAL__SORT_FILENAMES) {
        filenames.sort_unstable();
    }

    Ok(filenames)
}

fn adjust_relative_path(path: &str, new_cwd: &Path) -> Result<PathBuf, std::io::Error> {
    let absolute = std::path::absolute(path)?.clean();
    fs::relative_to(absolute, new_cwd)
}

/// Collect files to run hooks on.
/// Returns a list of file paths relative to the git root.
#[allow(clippy::too_many_arguments)]
async fn collect_files_from_args(
    git_root: &Path,
    workspace_root: &Path,
    hook_stage: Stage,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<String>,
    directories: Vec<String>,
) -> Result<Vec<PathBuf>> {
    if !hook_stage.operate_on_files() {
        return Ok(vec![]);
    }

    if let (Some(from_ref), Some(to_ref)) = (from_ref, to_ref) {
        let files = git::get_changed_files(&from_ref, &to_ref, workspace_root).await?;
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
                    "This file does not exist and will be ignored: `{}`",
                    non_exists[0]
                );
            } else {
                warn_user!(
                    "These files do not exist and will be ignored: `{}`",
                    non_exists.join(", ")
                );
            }
        }

        let mut exists = exists
            .into_iter()
            .map(|filename| adjust_relative_path(&filename, git_root).map(fs::normalize_path))
            .collect::<Result<FxHashSet<_>, _>>()?;

        for dir in directories {
            let dir = adjust_relative_path(&dir, git_root)?;
            let dir_files = git::ls_files(git_root, &dir).await?;
            for file in dir_files {
                let file = fs::normalize_path(file);
                exists.insert(file);
            }
        }

        debug!("Files passed as arguments: {}", exists.len());
        return Ok(exists.into_iter().collect());
    }

    if all_files {
        let files = git::ls_files(git_root, workspace_root).await?;
        debug!("All files in the workspace: {}", files.len());
        return Ok(files);
    }

    if git::is_in_merge_conflict().await? {
        let files = git::get_conflicted_files(workspace_root).await?;
        debug!("Conflicted files: {}", files.len());
        return Ok(files);
    }

    let files = git::get_staged_files(workspace_root).await?;
    debug!("Staged files: {}", files.len());

    Ok(files)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::GlobPatterns;

    fn glob_pattern(pattern: &str) -> FilePattern {
        FilePattern::Glob(GlobPatterns::new(vec![pattern.to_string()]).unwrap())
    }

    fn regex_pattern(pattern: &str) -> FilePattern {
        FilePattern::regex(pattern).unwrap()
    }

    #[test]
    fn filename_filter_supports_glob_include_and_exclude() {
        let include = glob_pattern("src/**/*.rs");
        let exclude = glob_pattern("src/**/ignored.rs");
        let filter = FilenameFilter::new(Some(&include), Some(&exclude));

        assert!(filter.matches(Path::new("src/lib/main.rs")));
        assert!(!filter.matches(Path::new("src/lib/ignored.rs")));
        assert!(!filter.matches(Path::new("tests/main.rs")));
    }

    #[cfg(unix)]
    #[test]
    fn filename_filter_allows_non_utf8_paths_without_patterns() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let path = Path::new(OsStr::from_bytes(b"bad-\xff.py"));
        let filter = FilenameFilter::new(None, None);

        assert!(filter.matches(path));
    }

    #[cfg(unix)]
    #[test]
    fn filename_filter_matches_non_utf8_paths_with_glob_patterns() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let include = glob_pattern("**/*.py");
        let exclude = glob_pattern("**/*.py");
        let path = Path::new(OsStr::from_bytes(b"bad-\xff.py"));
        let filter = FilenameFilter::new(Some(&include), None);

        assert!(filter.matches(path));

        let filter = FilenameFilter::new(None, Some(&exclude));

        assert!(!filter.matches(path));
    }

    #[cfg(unix)]
    #[test]
    fn filename_filter_skips_non_utf8_paths_with_regex_include() {
        use std::ffi::OsStr;
        use std::os::unix::ffi::OsStrExt as _;

        let include = regex_pattern(r".*\.py$");
        let path = Path::new(OsStr::from_bytes(b"bad-\xff.py"));
        let filter = FilenameFilter::new(Some(&include), None);

        assert!(!filter.matches(path));
    }
}
