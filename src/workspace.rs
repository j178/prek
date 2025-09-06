use std::borrow::Cow;
use std::fmt::Display;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use futures::StreamExt;
use ignore::WalkState;
use itertools::zip_eq;
use owo_colors::OwoColorize;
use rustc_hash::{FxHashMap, FxHashSet};
use thiserror::Error;
use tracing::{debug, error, instrument};

use crate::cli::run::Selectors;
use crate::config::{self, CONFIG_FILE, Config, ManifestHook, read_config};
use crate::fs::Simplified;
use crate::git::GIT_ROOT;
use crate::hook::{self, Hook, HookBuilder, Repo};
use crate::store::Store;
use crate::workspace::Error::MissingPreCommitConfig;
use crate::{git, store};

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error(transparent)]
    Config(#[from] config::Error),

    #[error(transparent)]
    Hook(#[from] hook::Error),

    #[error(transparent)]
    Git(#[from] anyhow::Error),

    #[error(
        "No `.pre-commit-config.yaml` found in the current directory or parent directories in the repository"
    )]
    MissingPreCommitConfig,

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
    depth: usize,
    config: Config,
    repos: Vec<Arc<Repo>>,
}

impl Display for Project {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.depth == 1 {
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
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.config_path.hash(state);
    }
}

impl Project {
    /// Initialize a new project from the configuration file with an optional root path.
    /// If root is not given, it will be the parent directory of the configuration file.
    pub(crate) fn from_config_file(
        config_path: Cow<'_, Path>,
        root: Option<PathBuf>,
    ) -> Result<Self, config::Error> {
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
            depth: 0,
            relative_path: PathBuf::new(),
            repos: Vec::with_capacity(size),
        })
    }

    /// Find the configuration file in the given path.
    pub(crate) fn from_directory(path: &Path) -> Result<Self, config::Error> {
        Self::from_config_file(path.join(CONFIG_FILE).into(), None)
    }

    /// Discover a project from the give path or search from the given path to the git root.
    pub(crate) fn discover(config_file: Option<&Path>, dir: &Path) -> Result<Project, Error> {
        let git_root = GIT_ROOT.as_ref().map_err(|e| Error::Git(e.into()))?;

        if let Some(config) = config_file {
            return Ok(Project::from_config_file(
                config.into(),
                Some(git_root.clone()),
            )?);
        }

        // TODO: add back `.pre-commit-config.yml` support
        // Walk from the given path up to the git root, to find the project root.
        let workspace_root = dir
            .ancestors()
            .take_while(|p| git_root.parent().map(|root| *p != root).unwrap_or(true))
            .find(|p| p.join(CONFIG_FILE).is_file())
            .ok_or(MissingPreCommitConfig)?
            .to_path_buf();

        debug!("Found project root at {}", workspace_root.user_display());
        Ok(Project::from_directory(&workspace_root)?)
    }

    fn with_relative_path(&mut self, relative_path: PathBuf) {
        self.relative_path = relative_path;
    }

    fn with_depth(&mut self, depth: usize) {
        self.depth = depth;
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

    /// Get the path to the project directory relative to the git root.
    ///
    /// Hooks will be executed in this directory and accept only files from this directory.
    /// In non-workspace mode (`--config <path>`), this is empty.
    pub(crate) fn relative_path(&self) -> &Path {
        &self.relative_path
    }

    pub(crate) fn depth(&self) -> usize {
        self.depth
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
                            repo: repo_config.repo.to_string(),
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
            }
        }

        Ok(hooks)
    }
}

pub(crate) struct Workspace {
    root: PathBuf,
    projects: Vec<Arc<Project>>,
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

        // TODO: add back `.pre-commit-config.yml` support
        // Walk from the given path up to the git root, to find the workspace root.
        let workspace_root = dir
            .ancestors()
            .take_while(|p| git_root.parent().map(|root| *p != root).unwrap_or(true))
            .find(|p| p.join(CONFIG_FILE).is_file())
            .ok_or(MissingPreCommitConfig)?
            .to_path_buf();

        debug!("Found workspace root at `{}`", workspace_root.display());
        Ok(workspace_root)
    }

    /// Discover the workspace from the given workspace root.
    #[instrument(level = "trace", skip(selectors))]
    pub(crate) fn discover(
        root: PathBuf,
        config: Option<PathBuf>,
        selectors: Option<&Selectors>,
    ) -> Result<Self, Error> {
        if let Some(config) = config {
            let project = Project::from_config_file(config.into(), Some(root.clone()))?;
            return Ok(Self {
                root,
                projects: vec![Arc::new(project)],
            });
        }

        // Walk subdirectories to find all projects.
        let projects = Mutex::new(Ok(Vec::new()));

        ignore::WalkBuilder::new(&root)
            .follow_links(false)
            .hidden(false) // Find from hidden directories.
            .build_parallel()
            .run(|| {
                Box::new(|result| {
                    let Ok(entry) = result else {
                        return WalkState::Continue;
                    };
                    let Some(file_type) = entry.file_type() else {
                        return WalkState::Continue;
                    };

                    // If it's a directory, check if it matches the selectors.
                    // Do not skip the root directory even if it doesn't match.
                    if file_type.is_dir() && entry.depth() > 0 {
                        let Some(selectors) = selectors.as_ref() else {
                            return WalkState::Continue;
                        };
                        let relative_path = entry
                            .path()
                            .strip_prefix(&root)
                            .expect("Entry path should be relative to the root");

                        if !selectors.matches_path(relative_path) {
                            debug!(
                                path = %relative_path.display(),
                                "Skipping unselected path"
                            );
                            return WalkState::Skip;
                        }
                    } else if file_type.is_file() && entry.file_name() == CONFIG_FILE {
                        match Project::from_config_file(entry.path().into(), None) {
                            Ok(mut project) => {
                                let depth = entry.depth();
                                let relative_path = entry
                                    .into_path()
                                    .parent()
                                    .and_then(|p| p.strip_prefix(&root).ok())
                                    .expect("Entry path should be relative to the root")
                                    .to_path_buf();
                                project.with_relative_path(relative_path);
                                project.with_depth(depth);

                                projects
                                    .lock()
                                    .unwrap()
                                    .as_mut()
                                    .unwrap()
                                    .push(Arc::new(project));
                            }
                            Err(config::Error::NotFound(_)) => {}
                            Err(e) => {
                                *projects.lock().unwrap() = Err(e);
                                return WalkState::Quit;
                            }
                        }
                    }

                    WalkState::Continue
                })
            });

        let mut projects = projects.into_inner().unwrap()?;
        debug_assert!(!projects.is_empty(), "At least one project should be found");

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
            Arc::get_mut(project).unwrap().with_idx(idx);
        }

        Ok(Self { root, projects })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
    }

    pub(crate) fn projects(&self) -> &[Arc<Project>] {
        &self.projects
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
                            repo: repo_config.repo.to_string(),
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
