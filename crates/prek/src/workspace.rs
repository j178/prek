use std::borrow::Cow;
use std::fmt::Display;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use anyhow::Result;
use futures::StreamExt;
use ignore::WalkState;
use itertools::zip_eq;
use owo_colors::OwoColorize;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML, PREK_TOML};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error, instrument, trace};

use crate::cli::run::Selectors;
use crate::config::{self, Config, ManifestHook, read_config};
use crate::fs::Simplified;
use crate::git::GIT_ROOT;
use crate::hook::{self, Hook, HookBuilder, Repo};
use crate::store::{CacheBucket, Store};
use crate::{git, store, warn_user};

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error(transparent)]
    Config(#[from] config::Error),

    #[error(transparent)]
    Hook(#[from] hook::Error),

    #[error(transparent)]
    Git(#[from] anyhow::Error),

    #[error(
        "No `prek.toml` or `.pre-commit-config.yaml` found in the current directory or parent directories.\n\n{} If you just added one, rerun your command with the `--refresh` flag to rescan the workspace.",
        "hint:".yellow().bold(),
    )]
    MissingConfigFile,

    #[error("Hook `{hook}` not present in repo `{repo}`")]
    HookNotFound { hook: String, repo: String },

    #[error("Failed to initialize repo `{repo}`")]
    Store {
        repo: String,
        #[source]
        error: Box<store::Error>,
    },
}

pub(crate) trait HookInitReporter {
    fn on_clone_start(&self, repo: &str) -> usize;
    fn on_clone_complete(&self, id: usize);
    fn on_complete(&self);
}

#[derive(Debug, Clone)]
pub(crate) struct Project {
    /// The absolute path of the project directory.
    root: PathBuf,
    /// The absolute path of the configuration file.
    config_path: PathBuf,
    /// The relative path of the project directory from the git root.
    relative_path: PathBuf,
    // The order index of the project in the workspace.
    idx: usize,
    config: Config,
    repos: Vec<Arc<Repo>>,
}

impl Display for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_root() {
            write!(f, ".")
        } else {
            write!(f, "{}", self.relative_path.display())
        }
    }
}

impl PartialEq for Project {
    fn eq(&self, other: &Self) -> bool {
        self.config_path == other.config_path
    }
}

impl Eq for Project {}

impl Hash for Project {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.config_path.hash(state);
    }
}

impl Project {
    /// Initialize a new project from the configuration file with an optional root path.
    /// If root is not given, it will be the parent directory of the configuration file.
    pub(crate) fn from_config_file(
        config_path: Cow<'_, Path>,
        root: Option<PathBuf>,
    ) -> Result<Self, Error> {
        debug!(
            path = %config_path.user_display(),
            "Loading project configuration"
        );

        let config = read_config(&config_path)?;
        let size = config.repos.len();

        let root = root.unwrap_or_else(|| {
            config_path
                .parent()
                .expect("config file must have a parent")
                .to_path_buf()
        });

        Ok(Self {
            root,
            config,
            config_path: config_path.into_owned(),
            idx: 0,
            relative_path: PathBuf::new(),
            repos: Vec::with_capacity(size),
        })
    }

    fn find_config(path: &Path) -> Option<PathBuf> {
        for name in [PREK_TOML, PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML] {
            let file = path.join(name);
            if file.is_file() {
                return Some(file);
            }
        }
        None
    }

    fn find_all_configs(path: &Path) -> Vec<(&'static str, PathBuf)> {
        let mut configs = Vec::new();
        for name in [PREK_TOML, PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML] {
            let file = path.join(name);
            if file.is_file() {
                configs.push((name, file));
            }
        }
        configs
    }

    /// Find the configuration file in the given path.
    pub(crate) fn from_directory(path: &Path) -> Result<Self, Error> {
        let present = Self::find_all_configs(path);

        let Some((_, selected)) = present.first() else {
            return Err(Error::MissingConfigFile);
        };

        if present.len() > 1 {
            let found = present
                .iter()
                .map(|(name, _)| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            warn_user!(
                "Multiple configuration files found ({found}); using `{selected}`",
                found = found,
                selected = selected.display(),
            );
        }

        Self::from_config_file(Cow::Borrowed(selected), None)
    }

    /// Discover a project from the give path or search from the given path to the git root.
    pub(crate) fn discover(config_file: Option<&Path>, dir: &Path) -> Result<Project, Error> {
        let git_root = GIT_ROOT.as_ref().map_err(|e| Error::Git(e.into()))?;

        if let Some(config) = config_file {
            return Project::from_config_file(config.into(), Some(git_root.clone()));
        }

        let workspace_root = Workspace::find_root(None, dir)?;
        debug!("Found project root at `{}`", workspace_root.user_display());

        Project::from_directory(&workspace_root)
    }

    pub(crate) fn with_relative_path(&mut self, relative_path: PathBuf) {
        self.relative_path = relative_path;
    }

    fn with_idx(&mut self, idx: usize) {
        self.idx = idx;
    }

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    /// Get the path to the configuration file.
    /// Must be an absolute path.
    pub(crate) fn config_file(&self) -> &Path {
        &self.config_path
    }

    /// Get the path to the project directory.
    pub(crate) fn path(&self) -> &Path {
        &self.root
    }

    /// Get the path to the project directory relative to the workspace root.
    ///
    /// Hooks will be executed in this directory and accept only files from this directory.
    /// In non-workspace mode (`--config <path>`), this is empty.
    pub(crate) fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    pub(crate) fn is_root(&self) -> bool {
        self.relative_path.as_os_str().is_empty()
    }

    pub(crate) fn depth(&self) -> usize {
        self.relative_path.components().count()
    }

    pub(crate) fn idx(&self) -> usize {
        self.idx
    }

    /// Initialize the project, cloning the repository and preparing hooks.
    pub(crate) async fn init_hooks(
        &mut self,
        store: &Store,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<Vec<Hook>, Error> {
        self.init_repos(store, reporter).await?;
        // TODO: avoid clone
        let project = Arc::new(self.clone());

        let hooks = project.internal_init_hooks().await?;

        Ok(hooks)
    }

    /// Initialize remote repositories for the project.
    #[allow(clippy::mutable_key_type)]
    async fn init_repos(
        &mut self,
        store: &Store,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<(), Error> {
        let remote_repos = Mutex::new(FxHashMap::default());

        let mut seen = FxHashSet::default();

        // Prepare remote repos in parallel.
        let remotes_iter = self.config.repos.iter().filter_map(|repo| match repo {
            // Deduplicate remote repos.
            config::Repo::Remote(repo) if seen.insert(repo) => Some(repo),
            _ => None,
        });

        let mut tasks =
            futures::stream::iter(remotes_iter)
                .map(async |repo_config| {
                    let path = store.clone_repo(repo_config, reporter).await.map_err(|e| {
                        Error::Store {
                            repo: repo_config.repo.clone(),
                            error: Box::new(e),
                        }
                    })?;

                    let repo = Arc::new(Repo::remote(
                        repo_config.repo.clone(),
                        repo_config.rev.clone(),
                        path,
                    )?);
                    remote_repos
                        .lock()
                        .unwrap()
                        .insert(repo_config, repo.clone());

                    Ok::<(), Error>(())
                })
                .buffer_unordered(5);

        while let Some(result) = tasks.next().await {
            result?;
        }

        drop(tasks);

        let remote_repos = remote_repos.into_inner().unwrap();
        let mut repos = Vec::with_capacity(self.config.repos.len());

        for repo in &self.config.repos {
            match repo {
                config::Repo::Remote(repo) => {
                    let repo = remote_repos.get(repo).expect("repo not found");
                    repos.push(repo.clone());
                }
                config::Repo::Local(repo) => {
                    let repo = Repo::local(repo.hooks.clone());
                    repos.push(Arc::new(repo));
                }
                config::Repo::Meta(repo) => {
                    let repo = Repo::meta(repo.hooks.clone());
                    repos.push(Arc::new(repo));
                }
                config::Repo::Builtin(repo) => {
                    let repo = Repo::builtin(repo.hooks.clone());
                    repos.push(Arc::new(repo));
                }
            }
        }

        self.repos = repos;

        Ok(())
    }

    /// Load and prepare hooks for the project.
    async fn internal_init_hooks(self: Arc<Self>) -> Result<Vec<Hook>, Error> {
        let mut hooks = Vec::new();

        for (repo_config, repo) in zip_eq(self.config.repos.iter(), self.repos.iter()) {
            match repo_config {
                config::Repo::Remote(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        // Check hook id is valid.
                        let Some(hook) = repo.get_hook(&hook_config.id) else {
                            return Err(Error::HookNotFound {
                                hook: hook_config.id.clone(),
                                repo: repo.to_string(),
                            });
                        };

                        let repo = Arc::clone(repo);
                        let mut builder =
                            HookBuilder::new(self.clone(), repo, hook.clone(), hooks.len());
                        builder.update(hook_config);
                        builder.combine(&self.config);

                        let hook = builder.build().await?;
                        hooks.push(hook);
                    }
                }
                config::Repo::Local(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Arc::clone(repo);
                        let mut builder =
                            HookBuilder::new(self.clone(), repo, hook_config.clone(), hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build().await?;
                        hooks.push(hook);
                    }
                }
                config::Repo::Meta(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Arc::clone(repo);
                        let hook_config = ManifestHook::from(hook_config.clone());
                        let mut builder =
                            HookBuilder::new(self.clone(), repo, hook_config, hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build().await?;
                        hooks.push(hook);
                    }
                }
                config::Repo::Builtin(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Arc::clone(repo);
                        let hook_config = ManifestHook::from(hook_config.clone());
                        let mut builder =
                            HookBuilder::new(self.clone(), repo, hook_config, hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build().await?;
                        hooks.push(hook);
                    }
                }
            }
        }

        Ok(hooks)
    }
}

/// Cache entry for a project configuration file
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CachedConfigFile {
    /// Absolute path to the config file
    path: PathBuf,
    /// Last modification time
    modified: SystemTime,
    /// File size for quick change detection
    size: u64,
}

/// Workspace discovery cache
#[derive(Debug, Clone, Serialize, Deserialize)]
struct WorkspaceCache {
    /// Cache version for compatibility
    version: u32,
    /// Workspace root path
    workspace_root: PathBuf,
    /// Cache creation timestamp
    created_at: SystemTime,
    /// Configuration files with their metadata
    config_files: Vec<CachedConfigFile>,
}

impl WorkspaceCache {
    const CURRENT_VERSION: u32 = 1;
    /// Maximum cache age before forcing rediscovery (1 hour)
    const MAX_CACHE_AGE: u64 = 60 * 60;

    /// Create a new cache from workspace discovery results
    fn new(workspace_root: PathBuf, projects: &[Project]) -> Self {
        let mut config_files = Vec::new();

        for project in projects {
            if let Ok(metadata) = std::fs::metadata(&project.config_path) {
                config_files.push(CachedConfigFile {
                    path: project.config_path.clone(),
                    modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                    size: metadata.len(),
                });
            }
        }

        Self {
            version: Self::CURRENT_VERSION,
            created_at: SystemTime::now(),
            workspace_root,
            config_files,
        }
    }

    /// Check if the cache is still valid
    fn is_valid(&self) -> bool {
        // Check cache age - invalidate if older than MAX_CACHE_AGE
        if let Ok(elapsed) = self.created_at.elapsed() {
            if elapsed.as_secs() > Self::MAX_CACHE_AGE {
                debug!(
                    "Cache is too old ({}s > {}s), invalidating",
                    elapsed.as_secs(),
                    Self::MAX_CACHE_AGE
                );
                return false;
            }
        }

        // Check if all config files still exist and haven't been modified
        for cached_file in &self.config_files {
            if let Ok(metadata) = std::fs::metadata(&cached_file.path) {
                let current_modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                let current_size = metadata.len();

                if current_modified != cached_file.modified || current_size != cached_file.size {
                    debug!(
                        path = %cached_file.path.display(),
                        "Config file changed, invalidating cache"
                    );
                    return false;
                }
            } else {
                debug!(
                    path = %cached_file.path.display(),
                    "Config file no longer exists, invalidating cache"
                );
                return false;
            }
        }

        // Check if workspace root still exists
        if !self.workspace_root.exists() {
            debug!("Workspace root no longer exists, invalidating cache");
            return false;
        }

        // Note: We don't check for newly added config files here to avoid
        // expensive directory traversal. New files will be detected when
        // the cache fails to load a project during cache restoration,
        // or when the cache expires due to age (every hour).

        true
    }

    /// Get cache file path for a workspace
    fn cache_path(store: &Store, workspace_root: &Path) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        workspace_root.hash(&mut hasher);
        let digest = hex::encode(hasher.finish().to_le_bytes());

        store
            .cache_path(CacheBucket::Prek)
            .join("workspace")
            .join(digest)
    }

    /// Load cache from file
    fn load(store: &Store, workspace_root: &Path, refresh: bool) -> Option<Self> {
        if refresh {
            return None;
        }
        let cache_path = Self::cache_path(store, workspace_root);

        match std::fs::read_to_string(&cache_path) {
            Ok(content) => match serde_json::from_str::<Self>(&content) {
                Ok(cache) => {
                    if cache.version == Self::CURRENT_VERSION && cache.is_valid() {
                        Some(cache)
                    } else {
                        // Invalid cache, remove it
                        let _ = std::fs::remove_file(&cache_path);
                        None
                    }
                }
                Err(e) => {
                    debug!("Failed to deserialize cache: {}", e);
                    let _ = std::fs::remove_file(&cache_path);
                    None
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                debug!("Failed to read cache file: {}", e);
                None
            }
        }
    }

    /// Save cache to file
    fn save(&self, store: &Store) -> Result<()> {
        let cache_path = Self::cache_path(store, &self.workspace_root);

        // Create cache directory if it doesn't exist
        if let Some(parent) = cache_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = serde_json::to_string_pretty(self)?;
        std::fs::write(&cache_path, content)?;
        Ok(())
    }
}

pub(crate) struct Workspace {
    root: PathBuf,
    projects: Vec<Arc<Project>>,
    all_projects: Vec<Project>,
}

impl Workspace {
    /// Find the workspace root.
    /// `dir` must be an absolute path.
    pub(crate) fn find_root(config_file: Option<&Path>, dir: &Path) -> Result<PathBuf, Error> {
        let git_root = GIT_ROOT.as_ref().map_err(|e| Error::Git(e.into()))?;

        if config_file.is_some() {
            // For `--config <path>`, the workspace root is the git root.
            return Ok(git_root.clone());
        }

        // Walk from the given path up to the git root, to find the workspace root.
        let workspace_root = dir
            .ancestors()
            .take_while(|p| git_root.parent().map(|root| *p != root).unwrap_or(true))
            .find(|p| Project::find_config(p).is_some())
            .ok_or(Error::MissingConfigFile)?
            .to_path_buf();

        debug!("Found workspace root at `{}`", workspace_root.display());
        Ok(workspace_root)
    }

    /// Discover the workspace from the given workspace root.
    #[instrument(level = "trace", skip(store, selectors))]
    pub(crate) fn discover(
        store: &Store,
        root: PathBuf,
        config: Option<PathBuf>,
        selectors: Option<&Selectors>,
        refresh: bool,
    ) -> Result<Self, Error> {
        if let Some(config) = config {
            let project = Project::from_config_file(config.into(), Some(root.clone()))?;
            let arc_project = Arc::new(project.clone());
            return Ok(Self {
                root,
                projects: vec![arc_project],
                all_projects: vec![project],
            });
        }

        // Try to load from cache first
        let projects = if let Some(cache) = WorkspaceCache::load(store, &root, refresh) {
            debug!("Loaded workspace from cache");
            let projects: Result<Vec<_>, _> = cache
                .config_files
                .into_iter()
                .map(
                    |config_file| match Project::from_config_file(config_file.path.into(), None) {
                        Ok(mut project) => {
                            let relative_path = project
                                .config_file()
                                .parent()
                                .and_then(|p| p.strip_prefix(&root).ok())
                                .expect("Entry path should be relative to the root")
                                .to_path_buf();
                            project.with_relative_path(relative_path);
                            Ok(project)
                        }
                        Err(e) => {
                            debug!("Failed to load cached project config: {}", e);
                            Err(e)
                        }
                    },
                )
                .collect();

            match projects {
                Ok(projects) if !projects.is_empty() => Some(projects),
                _ => {
                    debug!("Cache invalid or empty, performing fresh discovery");
                    None
                }
            }
        } else {
            None
        };

        let mut all_projects = if let Some(projects) = projects {
            projects
        } else {
            // Cache miss or invalid, perform fresh discovery
            debug!("Performing fresh workspace discovery");
            let projects = Self::discover_fresh(&root, selectors)?;

            // Save to cache
            let cache = WorkspaceCache::new(root.clone(), &projects);
            if let Err(e) = cache.save(store) {
                debug!("Failed to save workspace cache: {}", e);
            }
            projects
        };

        Self::sort_and_index_projects(&mut all_projects);

        let projects = if let Some(selectors) = selectors {
            let selected = all_projects
                .iter()
                .filter(|p| selectors.matches_path(p.relative_path()))
                .cloned()
                .map(Arc::new)
                .collect::<Vec<_>>();
            if selected.is_empty() {
                return Err(Error::MissingConfigFile);
            }
            selected
        } else {
            all_projects
                .iter()
                .cloned()
                .map(Arc::new)
                .collect::<Vec<_>>()
        };

        if projects.is_empty() {
            return Err(Error::MissingConfigFile);
        }

        Ok(Self {
            root,
            projects,
            all_projects,
        })
    }

    /// Perform fresh workspace discovery without cache
    fn discover_fresh(root: &Path, selectors: Option<&Selectors>) -> Result<Vec<Project>, Error> {
        let projects = Mutex::new(Ok(Vec::new()));

        let git_root = GIT_ROOT.as_ref().map_err(|e| Error::Git(e.into()))?;
        let submodules = git::list_submodules(git_root).unwrap_or_else(|e| {
            error!("Failed to list git submodules: {e}");
            Vec::new()
        });

        ignore::WalkBuilder::new(root)
            .follow_links(false)
            .add_custom_ignore_filename(".prekignore")
            .filter_entry(move |entry| {
                // Do not descend into git submodules.
                let Some(file_type) = entry.file_type() else {
                    return true;
                };
                if file_type.is_dir()
                    && submodules
                        .iter()
                        .any(|submodule| entry.path().starts_with(submodule))
                {
                    trace!(
                        path = %entry.path().user_display(),
                        "Skipping git submodule"
                    );
                    return false;
                }
                true
            })
            .build_parallel()
            .run(|| {
                Box::new(|result| {
                    let Ok(entry) = result else {
                        return WalkState::Continue;
                    };
                    if !entry
                        .file_type()
                        .is_some_and(|file_type| file_type.is_dir())
                    {
                        return WalkState::Continue;
                    }

                    match Project::from_directory(entry.path()) {
                        Ok(mut project) => {
                            let relative_path = entry
                                .into_path()
                                .strip_prefix(root)
                                .expect("Entry path should be relative to the root")
                                .to_path_buf();
                            project.with_relative_path(relative_path);

                            if let Ok(projects) = projects.lock().unwrap().as_mut() {
                                projects.push(project);
                            }
                        }
                        Err(Error::MissingConfigFile) => {}
                        Err(e) => {
                            // Exit early if the path is selected
                            if let Some(selectors) = selectors {
                                let relative_path = entry
                                    .path()
                                    .strip_prefix(root)
                                    .expect("Entry path should be relative to the root");
                                if selectors.matches_path(relative_path) {
                                    *projects.lock().unwrap() = Err(e);
                                    return WalkState::Quit;
                                }
                            }
                            // Otherwise, just log the error and continue
                            error!(
                                path = %entry.path().user_display(),
                                "Skipping project due to error: {e}"
                            );
                            return WalkState::Skip;
                        }
                    }

                    WalkState::Continue
                })
            });

        let projects = projects.into_inner().unwrap()?;
        if projects.is_empty() {
            return Err(Error::MissingConfigFile);
        }

        Ok(projects)
    }

    /// Sort projects by depth and assign indices
    fn sort_and_index_projects(projects: &mut [Project]) {
        // Sort projects by their depth in the directory tree.
        // The deeper the project comes first.
        // This is useful for nested projects where we want to prefer the most specific project.
        projects.sort_by(|a, b| {
            b.depth()
                .cmp(&a.depth())
                // If depth is the same, sort by relative path to have a deterministic order.
                .then_with(|| a.relative_path.cmp(&b.relative_path))
        });

        // Assign index to each project.
        for (idx, project) in projects.iter_mut().enumerate() {
            project.with_idx(idx);
        }
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn projects(&self) -> &[Arc<Project>] {
        &self.projects
    }

    pub(crate) fn all_projects(&self) -> &[Project] {
        &self.all_projects
    }

    /// Initialize remote repositories for all projects.
    async fn init_repos(
        &mut self,
        store: &Store,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<(), Error> {
        #[allow(clippy::mutable_key_type)]
        let remote_repos = {
            let remote_repos = Mutex::new(FxHashMap::default());

            let mut seen = FxHashSet::default();

            // Prepare remote repos in parallel.
            let remotes_iter = self
                .projects
                .iter()
                .flat_map(|proj| proj.config.repos.iter())
                .filter_map(|repo| match repo {
                    // Deduplicate remote repos.
                    config::Repo::Remote(repo) if seen.insert(repo) => Some(repo),
                    _ => None,
                })
                .cloned(); // TODO: avoid clone

            let mut tasks = futures::stream::iter(remotes_iter)
                .map(async |repo_config| {
                    let path = store
                        .clone_repo(&repo_config, reporter)
                        .await
                        .map_err(|e| Error::Store {
                            repo: repo_config.repo.clone(),
                            error: Box::new(e),
                        })?;

                    let repo = Arc::new(Repo::remote(
                        repo_config.repo.clone(),
                        repo_config.rev.clone(),
                        path,
                    )?);
                    remote_repos
                        .lock()
                        .unwrap()
                        .insert(repo_config, repo.clone());

                    Ok::<(), Error>(())
                })
                .buffer_unordered(5);

            while let Some(result) = tasks.next().await {
                result?;
            }

            drop(tasks);

            remote_repos.into_inner().unwrap()
        };

        for project in &mut self.projects {
            let mut repos = Vec::with_capacity(project.config.repos.len());

            for repo in &project.config.repos {
                match repo {
                    config::Repo::Remote(repo) => {
                        let repo = remote_repos.get(repo).expect("repo not found");
                        repos.push(repo.clone());
                    }
                    config::Repo::Local(repo) => {
                        let repo = Repo::local(repo.hooks.clone());
                        repos.push(Arc::new(repo));
                    }
                    config::Repo::Meta(repo) => {
                        let repo = Repo::meta(repo.hooks.clone());
                        repos.push(Arc::new(repo));
                    }
                    config::Repo::Builtin(repo) => {
                        let repo = Repo::builtin(repo.hooks.clone());
                        repos.push(Arc::new(repo));
                    }
                }
            }

            Arc::get_mut(project).unwrap().repos = repos;
        }

        Ok(())
    }

    /// Load and prepare hooks for all projects.
    pub(crate) async fn init_hooks(
        &mut self,
        store: &Store,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<Vec<Hook>, Error> {
        self.init_repos(store, reporter).await?;

        let mut hooks = Vec::new();
        for project in &self.projects {
            let project_hooks = Arc::clone(project).internal_init_hooks().await?;
            hooks.extend(project_hooks);
        }

        reporter.map(HookInitReporter::on_complete);

        Ok(hooks)
    }

    /// Check if all configuration files are staged in git.
    pub(crate) async fn check_configs_staged(&self) -> Result<()> {
        let config_files = self
            .projects
            .iter()
            .map(|project| project.config_file())
            .collect::<Vec<_>>();
        let non_staged = git::files_not_staged(&config_files).await?;

        let git_root = GIT_ROOT.as_ref()?;
        if !non_staged.is_empty() {
            let non_staged = non_staged
                .into_iter()
                .map(|p| git_root.join(p))
                .collect::<Vec<_>>();
            match non_staged.as_slice() {
                [filename] => anyhow::bail!(
                    "prek configuration file is not staged, run `{}` to stage it",
                    format!("git add {}", filename.user_display()).cyan()
                ),
                _ => anyhow::bail!(
                    "The following configuration files are not staged, `git add` them first:\n{}",
                    non_staged
                        .iter()
                        .map(|p| format!("  {}", p.user_display()))
                        .collect::<Vec<_>>()
                        .join("\n")
                ),
            }
        }

        Ok(())
    }
}
