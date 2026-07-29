use std::cell::OnceCell;
use std::ffi::OsStr;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use globset::Glob;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use prek_identify::{TagSet, tags_from_path};
use rustc_hash::{FxHashMap, FxHashSet};
use tracing::{debug, error, instrument};

use crate::config::{FilePattern, GlobPatterns, Stage};
use crate::fs::PathClean;
use crate::git::GIT_ROOT;
use crate::hook::Hook;
use crate::repo;
use crate::workspace::Project;
use crate::{fs, warn_user};

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
    /// Create a tag filter from a hook's type selectors.
    pub(crate) fn new(
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

    /// Return whether a project-owned file passes this hook's file and tag filters.
    pub(crate) fn matches_project_file<'p>(
        &self,
        file: &ProjectFile<'p>,
        tag_cache: &FileTagCache<'p>,
    ) -> bool {
        self.matches_filename(file.hook_path) && self.matches_tags(file.tags(tag_cache))
    }
}

/// A workspace file after project ownership and project-level filters have been applied.
pub(crate) struct ProjectFile<'a> {
    file_idx: usize,
    hook_path: &'a Path,
}

impl<'a> ProjectFile<'a> {
    fn new(file_idx: usize, hook_path: &'a Path) -> Self {
        Self {
            file_idx,
            hook_path,
        }
    }

    /// Return the path relative to the owning project, which is what hook patterns match.
    pub(crate) fn hook_path(&self) -> &Path {
        self.hook_path
    }

    /// Return cached tags for the workspace-relative path.
    pub(crate) fn tags<'cache>(
        &self,
        tag_cache: &'cache FileTagCache<'a>,
    ) -> Option<&'cache TagSet> {
        tag_cache.tags(self.file_idx)
    }
}

#[derive(Default)]
pub(crate) struct FileTagCache<'a> {
    paths: &'a [PathBuf],
    tags_by_file: Vec<OnceCell<Option<TagSet>>>,
}

impl<'a> FileTagCache<'a> {
    pub(crate) fn from_paths(paths: &'a [PathBuf]) -> Self {
        let tags_by_file = (0..paths.len()).map(|_| OnceCell::new()).collect();
        Self {
            paths,
            tags_by_file,
        }
    }

    pub(crate) fn tags(&self, file_idx: usize) -> Option<&TagSet> {
        self.tags_by_file[file_idx]
            .get_or_init(|| {
                let path = &self.paths[file_idx];
                match tags_from_path(path) {
                    Ok(tags) => Some(tags),
                    Err(err) => {
                        error!(filename = ?path.display(), error = %err, "Failed to get tags");
                        None
                    }
                }
            })
            .as_ref()
    }
}

pub(crate) struct ProjectFiles<'a> {
    files: Vec<ProjectFile<'a>>,
}

impl<'a> ProjectFiles<'a> {
    fn with_capacity(capacity: usize) -> Self {
        Self {
            files: Vec::with_capacity(capacity),
        }
    }

    fn push(&mut self, file_idx: usize, hook_path: &'a Path) {
        self.files.push(ProjectFile::new(file_idx, hook_path));
    }

    /// Visit project-owned files without collecting them.
    ///
    /// This applies the same ownership, orphan-project, and project-level filtering rules as the
    /// run file index. Return [`ControlFlow::Break`] from `visit` to stop calling the visitor.
    /// Orphan projects still finish marking owned files as consumed before returning.
    pub(crate) fn visit_for_project<I, F>(
        filenames: I,
        project: &Project,
        consumed_files: Option<&FxHashSet<&'a Path>>,
        mut newly_consumed_files: Option<&mut FxHashSet<&'a Path>>,
        mut visit: F,
    ) where
        I: Iterator<Item = &'a PathBuf> + Send,
        F: FnMut(ProjectFile<'a>) -> ControlFlow<()>,
    {
        let filename_filter = FilenameFilter::new(
            project.config().files.as_ref(),
            project.config().exclude.as_ref(),
        );
        let relative_path = project.relative_path();
        let orphan = project.config().orphan.unwrap_or(false);
        let must_finish_consuming = orphan && newly_consumed_files.is_some();
        let mut visiting = true;

        // The order of below filters matters.
        // If this is an orphan project, we must mark all files in its directory as consumed
        // *before* applying the project's include/exclude patterns. This ensures that even
        // files excluded by this project are still considered "owned" by it and hidden
        // from parent projects.
        for (file_idx, filename) in filenames.enumerate() {
            // Collect files that are inside the hook project directory.
            if !filename.starts_with(relative_path) {
                continue;
            }

            // Skip files that have already been consumed by subprojects.
            if consumed_files
                .is_some_and(|consumed_files| consumed_files.contains(filename.as_path()))
            {
                continue;
            }

            // Consume this file in current orphan project, so it won't be visited by parent projects.
            if orphan {
                if let Some(newly_consumed_files) = newly_consumed_files.as_mut() {
                    if !newly_consumed_files.insert(filename) {
                        continue;
                    }
                }
            }

            if !visiting {
                continue;
            }

            // Strip the project-relative prefix before applying project-level include/exclude patterns.
            let relative = filename
                .strip_prefix(relative_path)
                .expect("Filename should start with project relative path");
            if filename_filter.matches(relative)
                && visit(ProjectFile::new(file_idx, relative)).is_break()
            {
                if must_finish_consuming {
                    visiting = false;
                } else {
                    break;
                }
            }
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.files.len()
    }

    pub(crate) fn iter(&self) -> impl Iterator<Item = &ProjectFile<'a>> {
        self.files.iter()
    }

    /// Filter filenames by file patterns and tags for a specific hook.
    #[instrument(level = "trace", skip_all, fields(hook = ?hook.id))]
    pub(crate) fn matching_filenames(
        &self,
        hook: &Hook,
        tag_cache: &FileTagCache<'a>,
    ) -> Vec<&'a Path> {
        let hook_filter = HookFileFilter::new(hook);
        let mut filenames = Vec::new();
        for file in &self.files {
            if hook_filter.matches_project_file(file, tag_cache) {
                filenames.push(file.hook_path);
            }
        }
        filenames
    }

    /// Return whether at least one file matches a hook without collecting every filename.
    pub(crate) fn has_matching_file(&self, hook: &Hook, tag_cache: &FileTagCache<'a>) -> bool {
        let hook_filter = HookFileFilter::new(hook);
        for file in &self.files {
            if hook_filter.matches_project_file(file, tag_cache) {
                return true;
            }
        }
        false
    }
}

#[derive(Default)]
struct ProjectPathNode<'a> {
    project_idx: Option<usize>,
    children: FxHashMap<&'a OsStr, ProjectPathNode<'a>>,
}

impl<'a> ProjectPathNode<'a> {
    fn insert(&mut self, path: &'a Path, project_idx: usize) {
        let mut node = self;
        for component in path.components() {
            node = node.children.entry(component.as_os_str()).or_default();
        }
        let previous = node.project_idx.replace(project_idx);
        debug_assert!(previous.is_none());
    }

    fn matching_projects(&self, path: &Path, matches: &mut Vec<usize>) {
        matches.clear();

        let mut node = self;
        if let Some(project_idx) = node.project_idx {
            matches.push(project_idx);
        }
        if node.children.is_empty() {
            return;
        }
        for component in path.components() {
            let Some(child) = node.children.get(component.as_os_str()) else {
                break;
            };
            node = child;
            if let Some(project_idx) = node.project_idx {
                matches.push(project_idx);
            }
            if node.children.is_empty() {
                break;
            }
        }
    }
}

/// Project-relative views of the run input, built once and shared by hook setup and execution.
pub(crate) struct RunFileIndex<'a> {
    projects: Vec<ProjectFiles<'a>>,
    tag_cache: FileTagCache<'a>,
}

impl<'a> RunFileIndex<'a> {
    pub(crate) fn new(input: &'a RunInput, projects: &[Arc<Project>]) -> Self {
        let RunInput::Files(filenames) = input else {
            return Self {
                projects: Vec::new(),
                tag_cache: FileTagCache::default(),
            };
        };

        debug_assert!(
            projects
                .iter()
                .enumerate()
                .all(|(idx, project)| project.idx() == idx),
            "workspace projects must be indexed in storage order"
        );

        let mut project_tree = ProjectPathNode::default();
        let project_filters = projects
            .iter()
            .map(|project| {
                project_tree.insert(project.relative_path(), project.idx());
                FilenameFilter::new(
                    project.config().files.as_ref(),
                    project.config().exclude.as_ref(),
                )
            })
            .collect::<Vec<_>>();
        let mut project_files = projects
            .iter()
            .map(|project| {
                ProjectFiles::with_capacity(if project.is_root() {
                    filenames.len()
                } else {
                    0
                })
            })
            .collect::<Vec<_>>();

        let mut matching_projects = Vec::new();
        for (file_idx, filename) in filenames.iter().enumerate() {
            project_tree.matching_projects(filename, &mut matching_projects);

            // The tree yields ancestors from root to leaf. Apply ownership from the most
            // specific project upwards, stopping once an orphan project consumes the file.
            for &project_idx in matching_projects.iter().rev() {
                let project = &projects[project_idx];
                let hook_path = filename
                    .strip_prefix(project.relative_path())
                    .expect("matched project path must be a file prefix");
                if project_filters[project_idx].matches(hook_path) {
                    project_files[project_idx].push(file_idx, hook_path);
                }
                if project.config().orphan.unwrap_or(false) {
                    break;
                }
            }
        }

        Self {
            projects: project_files,
            tag_cache: FileTagCache::from_paths(filenames),
        }
    }

    pub(crate) fn project_files(&self, project: &Project) -> &ProjectFiles<'a> {
        &self.projects[project.idx()]
    }

    pub(crate) fn tag_cache(&self) -> &FileTagCache<'a> {
        &self.tag_cache
    }
}

#[derive(Debug, Default)]
pub(crate) enum FileSelection {
    #[default]
    Default,
    All {
        from_ref: Option<String>,
        to_ref: Option<String>,
    },
    Diff {
        from_ref: String,
        to_ref: String,
    },
    Explicit {
        files: Vec<String>,
        globs: Vec<Glob>,
        directories: Vec<String>,
    },
}

impl FileSelection {
    pub(crate) const fn requires_clean_worktree(&self) -> bool {
        matches!(self, Self::Default | Self::Diff { .. })
    }

    pub(crate) fn refs(&self) -> (Option<&str>, Option<&str>) {
        match self {
            Self::Diff { from_ref, to_ref } => (Some(from_ref), Some(to_ref)),
            Self::All { from_ref, to_ref } => (from_ref.as_deref(), to_ref.as_deref()),
            Self::Default | Self::Explicit { .. } => (None, None),
        }
    }
}

#[derive(Default)]
pub(crate) struct CollectOptions {
    pub(crate) input_mode: RunInputMode,
    pub(crate) selection: FileSelection,
    pub(crate) commit_msg_filename: Option<String>,
}

impl CollectOptions {
    pub(crate) fn all_files() -> Self {
        Self {
            selection: FileSelection::All {
                from_ref: None,
                to_ref: None,
            },
            ..Default::default()
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum RunInputMode {
    #[default]
    Files,
    MessageFile,
    NoFiles,
}

impl From<Stage> for RunInputMode {
    fn from(stage: Stage) -> Self {
        match stage {
            Stage::CommitMsg | Stage::PrepareCommitMsg => Self::MessageFile,
            Stage::Manual | Stage::PreCommit | Stage::PreMergeCommit | Stage::PrePush => {
                Self::Files
            }
            Stage::PostCheckout
            | Stage::PostCommit
            | Stage::PostMerge
            | Stage::PostRewrite
            | Stage::PreRebase => Self::NoFiles,
        }
    }
}

pub(crate) enum RunInput {
    /// File paths relative to the workspace root.
    Files(Vec<PathBuf>),
    /// Absolute path to the Git message file passed by `commit-msg` and `prepare-commit-msg`.
    MessageFile(PathBuf),
}

impl RunInput {
    /// Return workspace-relative file paths.
    ///
    /// `MessageFile` inputs are hook arguments, not workspace files, so this
    /// compatibility helper discards them and returns an empty list.
    pub(crate) fn into_files(self) -> Vec<PathBuf> {
        match self {
            Self::Files(files) => files,
            Self::MessageFile(_) => vec![],
        }
    }
}

/// Get hook input for the selected input mode.
pub(crate) async fn collect_run_input(root: &Path, opts: CollectOptions) -> Result<RunInput> {
    let CollectOptions {
        input_mode,
        selection,
        commit_msg_filename,
    } = opts;

    let git_root = GIT_ROOT.as_ref()?;

    match input_mode {
        RunInputMode::Files => {}
        RunInputMode::MessageFile => {
            let path = commit_msg_filename.expect("commit_msg_filename should be set");
            return Ok(RunInput::MessageFile(git_root.join(path)));
        }
        RunInputMode::NoFiles => return Ok(RunInput::Files(vec![])),
    }

    // The workspace root relative to the git root.
    let relative_root = root.strip_prefix(git_root).with_context(|| {
        format!(
            "Workspace root `{}` is not under git root `{}`",
            root.display(),
            git_root.display()
        )
    })?;

    let filenames = collect_files_for_selection(git_root, root, selection).await?;

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
    if EnvVars.is_set(EnvVars::PREK_INTERNAL__SORT_FILENAMES) {
        filenames.sort_unstable();
    }

    Ok(RunInput::Files(filenames))
}

fn adjust_relative_path(path: &str, new_cwd: &Path) -> Result<PathBuf, std::io::Error> {
    let absolute = std::path::absolute(path)?.clean();
    fs::relative_to(absolute, new_cwd)
}

fn warn_missing_files(files: &[String]) {
    match files {
        [] => {}
        [file] => {
            warn_user!("This file does not exist and will be ignored: `{file}`");
        }
        files => {
            warn_user!(
                "These files do not exist and will be ignored: `{}`",
                files.join(", ")
            );
        }
    }
}

fn collect_file_arguments(files: Vec<String>, git_root: &Path) -> Result<FxHashSet<PathBuf>> {
    let mut selected = FxHashSet::default();
    let mut missing = Vec::new();

    for file in files {
        if fs_err::exists(&file)? {
            selected.insert(fs::normalize_path(adjust_relative_path(&file, git_root)?));
        } else {
            missing.push(file);
        }
    }

    warn_missing_files(&missing);
    Ok(selected)
}

fn git_pathspec(path: &Path) -> &Path {
    if path.as_os_str().is_empty() {
        Path::new(".")
    } else {
        path
    }
}

async fn collect_explicit_files(
    git_root: &Path,
    files: Vec<String>,
    globs: Vec<Glob>,
    directories: Vec<String>,
) -> Result<Vec<PathBuf>> {
    let mut selected = collect_file_arguments(files, git_root)?;
    let patterns = GlobPatterns::from_globs(globs)?;
    let cwd_relative = if patterns.is_empty() {
        PathBuf::new()
    } else {
        fs::normalize_path(adjust_relative_path(".", git_root)?)
    };
    let directories = directories
        .into_iter()
        .map(|directory| adjust_relative_path(&directory, git_root).map(fs::normalize_path))
        .collect::<Result<Vec<_>, _>>()?;

    let mut pathspecs = directories
        .iter()
        .map(|directory| git_pathspec(directory))
        .collect::<Vec<_>>();
    if !patterns.is_empty() {
        pathspecs.push(git_pathspec(&cwd_relative));
    }

    if !pathspecs.is_empty() {
        for file in repo::ls_files(git_root, pathspecs).await? {
            let file = fs::normalize_path(file);
            let matches_directory = directories
                .iter()
                .any(|directory| directory.as_os_str().is_empty() || file.starts_with(directory));
            let matches_glob = !matches_directory
                && !patterns.is_empty()
                && file
                    .strip_prefix(&cwd_relative)
                    .is_ok_and(|relative| patterns.is_match(relative));

            if matches_directory || matches_glob {
                selected.insert(file);
            }
        }
    }

    debug!("Files passed as arguments: {}", selected.len());
    Ok(selected.into_iter().collect())
}

/// Collect files to run hooks on.
/// Returns a list of file paths relative to the git root.
async fn collect_files_for_selection(
    git_root: &Path,
    workspace_root: &Path,
    selection: FileSelection,
) -> Result<Vec<PathBuf>> {
    match selection {
        FileSelection::Diff { from_ref, to_ref } => {
            let files = repo::changed_files_between(&from_ref, &to_ref, workspace_root).await?;
            debug!(
                "Files changed between {} and {}: {}",
                from_ref,
                to_ref,
                files.len()
            );
            Ok(files)
        }
        FileSelection::Explicit {
            files,
            globs,
            directories,
        } => collect_explicit_files(git_root, files, globs, directories).await,
        FileSelection::All { .. } => {
            let files = repo::ls_files(git_root, [workspace_root]).await?;
            debug!("All files in the workspace: {}", files.len());
            Ok(files)
        }
        FileSelection::Default => {
            if let Some(files) = repo::conflicted_files(workspace_root).await? {
                debug!("Conflicted files: {}", files.len());
                return Ok(files);
            }

            let files = repo::default_files(workspace_root).await?;
            debug!("Default files from repository backend: {}", files.len());
            Ok(files)
        }
    }
}

pub(super) const fn stage_uses_message_file_input(stage: Stage) -> bool {
    matches!(stage, Stage::CommitMsg | Stage::PrepareCommitMsg)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::FileSelectionArgs;
    use crate::config::GlobPatterns;

    #[test]
    fn all_file_selection_preserves_partial_refs() {
        let selection: FileSelection = FileSelectionArgs {
            all_files: true,
            to_ref: Some("local-sha".to_string()),
            ..Default::default()
        }
        .into();

        assert_eq!(selection.refs(), (None, Some("local-sha")));
    }

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

    #[test]
    fn project_path_tree_matches_nested_ancestors() {
        let project_paths = [
            PathBuf::new(),
            PathBuf::from("src"),
            PathBuf::from("src/backend"),
        ];
        let mut tree = ProjectPathNode::default();
        for (idx, path) in project_paths.iter().enumerate() {
            tree.insert(path, idx);
        }

        let mut matches = Vec::new();
        tree.matching_projects(Path::new("src/backend/lib.rs"), &mut matches);

        assert_eq!(matches, vec![0, 1, 2]);
    }

    #[test]
    fn project_path_tree_respects_component_boundaries() {
        let project_paths = [PathBuf::new(), PathBuf::from("src")];
        let mut tree = ProjectPathNode::default();
        for (idx, path) in project_paths.iter().enumerate() {
            tree.insert(path, idx);
        }

        let mut matches = Vec::new();
        tree.matching_projects(Path::new("src-tools/main.rs"), &mut matches);

        assert_eq!(matches, vec![0]);
    }
}
