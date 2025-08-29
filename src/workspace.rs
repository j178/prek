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
use tracing::{debug, error, trace};

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
    /// Initialize a new project from the configuration file or the file in the current working directory.
    pub(crate) fn from_config_file(config_path: PathBuf) -> Result<Self, config::Error> {
        debug!(
            path = %config_path.user_display(),
            "Loading project configuration"
        );
        let config = read_config(&config_path)?;
        let size = config.repos.len();
        Ok(Self {
            config,
            config_path,
            idx: 0,
            depth: 0,
            relative_path: PathBuf::new(),
            repos: Vec::with_capacity(size),
        })
    }

    /// Find the configuration file in the given path.
    pub(crate) fn from_directory(path: &Path) -> Result<Self, config::Error> {
        Self::from_config_file(path.join(CONFIG_FILE))
    }

    // Find the project configuration file in the current working directory or its ancestors.
    //
    // This function will traverse up the directory tree from the given path until the git root.
    // pub(crate) fn from_directory_ancestors(path: &Path) -> Result<Self, Error> {
    //     let mut current = path.to_path_buf();
    //     loop {
    //         match Self::from_directory(&current) {
    //             Ok(project) => return Ok(project),
    //             Err(Error::InvalidConfig(config::Error::NotFound(_))) => {
    //                 if let Some(parent) = current.parent() {
    //                     current = parent.to_path_buf();
    //                 } else {
    //                     break;
    //                 }
    //             }
    //             Err(e) => return Err(e),
    //         }
    //     }
    //     Err(Error::InvalidConfig(config::Error::NotFound(
    //         CWD.user_display().to_string(),
    //     )))
    // }

    /// Initialize a new project from the configuration file or find it in the given path.
    pub(crate) fn from_config_file_or_directory(
        config: Option<PathBuf>,
        path: &Path,
    ) -> Result<Self, config::Error> {
        if let Some(config) = config {
            return Self::from_config_file(config);
        }
        Self::from_directory(path)
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
        self.config_path
            .parent()
            .expect("Project path should have a parent")
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

#[derive(Default)]
pub(crate) struct DiscoverOptions {
    config: Option<PathBuf>,
    directory: Option<PathBuf>,
}

impl DiscoverOptions {
    pub(crate) fn new(config: Option<PathBuf>, path: &Path) -> Self {
        Self {
            config,
            directory: Some(path.to_path_buf()),
        }
    }

    pub(crate) fn config(mut self, config: PathBuf) -> Self {
        self.config = Some(config);
        self
    }

    pub(crate) fn directory(mut self, directory: PathBuf) -> Self {
        self.directory = Some(directory);
        self
    }
}

impl Workspace {
    pub(crate) fn discover(opts: DiscoverOptions) -> Result<Self, Error> {
        if let Some(config) = opts.config {
            let project = Project::from_config_file(config)?;
            return Ok(Self {
                root: project.path().to_path_buf(),
                projects: vec![Arc::new(project)],
            });
        }

        // directory should be absolute
        let Some(path) = opts.directory else {
            panic!("Config file or path must be provided to discover workspace");
        };

        let git_root = GIT_ROOT.as_ref().map_err(|e| Error::Git(e.into()))?;

        // Walk from the given path up to the git root, to find the workspace root.
        let workspace_root = path
            .ancestors()
            .take_while(|p| git_root.parent().map(|root| *p != root).unwrap_or(true))
            .find(|p| p.join(CONFIG_FILE).is_file())
            .ok_or(MissingPreCommitConfig)?
            .to_path_buf();

        trace!("Found workspace root at {}", workspace_root.user_display());

        // Then walk subdirectories to find all projects.
        let projects = Mutex::new(Ok(Vec::new()));

        ignore::WalkBuilder::new(&workspace_root)
            .follow_links(false)
            .hidden(false) // Find from hidden directories.
            .build_parallel()
            .run(|| {
                Box::new(|result| {
                    if let Ok(entry) = result {
                        if entry.file_type().is_some_and(|t| t.is_file())
                            && entry.file_name() == CONFIG_FILE
                        {
                            match Project::from_config_file(entry.path().to_path_buf()) {
                                Ok(mut project) => {
                                    let depth = entry.depth();
                                    let relative_path = entry
                                        .into_path()
                                        .parent()
                                        .and_then(|p| p.strip_prefix(&workspace_root).ok())
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
                    }

                    WalkState::Continue
                })
            });

        let mut projects = projects.into_inner().unwrap()?;

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

        Ok(Self {
            root: workspace_root,
            projects,
        })
    }

    pub(crate) fn root(&self) -> &Path {
        &self.root
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

    pub(crate) async fn check_config_staged(&self) -> Result<()> {
        let config_files = self
            .projects
            .iter()
            .map(|project| project.config_file())
            .collect::<Vec<_>>();
        let non_staged = git::files_not_staged(&config_files).await?;

        if !non_staged.is_empty() {
            if non_staged.len() == 1 {
                anyhow::bail!(
                    "prek configuration file is not staged, run `{}` to stage it",
                    format!("git add {}", non_staged[0].user_display()).cyan()
                )
            }
            anyhow::bail!(
                "The following configuration files are not staged, `git add` them first:\n{}",
                non_staged
                    .iter()
                    .map(|p| format!("  {}", p.user_display()))
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_workspace_discovery_empty_directory() -> Result<()> {
        let dir = tempfile::tempdir()?;

        let workspace =
            Workspace::discover(DiscoverOptions::default().directory(dir.path().to_path_buf()))?;
        assert_eq!(workspace.projects.len(), 0);

        Ok(())
    }

    #[test]
    fn test_workspace_discovery_single_project() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // Create a project with .pre-commit-config.yaml
        let project_dir = dir.path().join("project1");
        fs::create_dir(&project_dir)?;
        fs::write(
            project_dir.join(".pre-commit-config.yaml"),
            r#"
repos:
  - repo: local
    hooks:
      - id: test-hook
        name: Test Hook
        entry: echo "test"
        language: system
"#,
        )?;

        let workspace =
            Workspace::discover(DiscoverOptions::default().directory(dir.path().to_path_buf()))?;
        assert_eq!(workspace.projects.len(), 1);
        assert_eq!(
            workspace.projects[0].config_file(),
            project_dir.join(".pre-commit-config.yaml")
        );

        Ok(())
    }

    #[test]
    fn test_workspace_discovery_multiple_projects() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // Create multiple projects
        for i in 1..=3 {
            let project_dir = dir.path().join(format!("project{i}"));
            fs::create_dir(&project_dir)?;
            fs::write(
                project_dir.join(".pre-commit-config.yaml"),
                r#"
repos:
  - repo: local
    hooks:
      - id: test-hook
        name: Test Hook
        entry: echo "test"
        language: system
"#,
            )?;
        }

        let workspace =
            Workspace::discover(DiscoverOptions::default().directory(dir.path().to_path_buf()))?;
        assert_eq!(workspace.projects.len(), 3);

        // Verify all projects were found
        let config_files: Vec<_> = workspace
            .projects
            .iter()
            .map(|p| p.config_file().file_name().unwrap().to_str().unwrap())
            .collect();
        assert!(
            config_files
                .iter()
                .all(|&name| name == ".pre-commit-config.yaml")
        );

        Ok(())
    }

    #[test]
    fn test_workspace_discovery_nested_projects() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // Create nested project structure
        let parent_project = dir.path().join("parent");
        let nested_project = parent_project.join("nested");
        fs::create_dir_all(&nested_project)?;

        // Parent project
        fs::write(
            parent_project.join(".pre-commit-config.yaml"),
            r#"
repos:
  - repo: local
    hooks:
      - id: parent-hook
        name: Parent Hook
        entry: echo "parent"
        language: system
"#,
        )?;

        // Nested project
        fs::write(
            nested_project.join(".pre-commit-config.yaml"),
            r#"
repos:
  - repo: local
    hooks:
      - id: nested-hook
        name: Nested Hook
        entry: echo "nested"
        language: system
"#,
        )?;

        let workspace =
            Workspace::discover(DiscoverOptions::default().directory(dir.path().to_path_buf()))?;
        assert_eq!(workspace.projects.len(), 2);

        Ok(())
    }

    #[test]
    fn test_workspace_discovery_mixed_config_files() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // Create project with .pre-commit-config.yaml
        let project1 = dir.path().join("project1");
        fs::create_dir(&project1)?;
        fs::write(
            project1.join(".pre-commit-config.yaml"),
            r#"
repos:
  - repo: local
    hooks:
      - id: test-hook1
        name: Test Hook 1
        entry: echo "test1"
        language: system
"#,
        )?;

        // Create project with .pre-commit-config.yml
        let project2 = dir.path().join("project2");
        fs::create_dir(&project2)?;
        fs::write(
            project2.join(".pre-commit-config.yml"),
            r#"
repos:
  - repo: local
    hooks:
      - id: test-hook2
        name: Test Hook 2
        entry: echo "test2"
        language: system
"#,
        )?;

        let workspace =
            Workspace::discover(DiscoverOptions::default().directory(dir.path().to_path_buf()))?;
        assert_eq!(workspace.projects.len(), 2);

        Ok(())
    }

    #[test]
    fn test_workspace_discovery_invalid_config() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // Create a project with invalid YAML
        let project_dir = dir.path().join("invalid_project");
        fs::create_dir(&project_dir)?;
        fs::write(
            project_dir.join(".pre-commit-config.yaml"),
            "invalid: yaml: content: [unclosed",
        )?;

        // Should return an error for invalid config
        let result =
            Workspace::discover(DiscoverOptions::default().directory(dir.path().to_path_buf()));
        assert!(result.is_err());

        Ok(())
    }

    #[test]
    fn test_workspace_discovery_prefers_yaml_over_yml() -> Result<()> {
        let dir = tempfile::tempdir()?;

        // Create project with both .yaml and .yml files
        let project_dir = dir.path().join("project");
        fs::create_dir(&project_dir)?;

        fs::write(
            project_dir.join(".pre-commit-config.yaml"),
            r#"
repos:
  - repo: local
    hooks:
      - id: yaml-hook
        name: YAML Hook
        entry: echo "yaml"
        language: system
"#,
        )?;

        fs::write(
            project_dir.join(".pre-commit-config.yml"),
            r#"
repos:
  - repo: local
    hooks:
      - id: yml-hook
        name: YML Hook
        entry: echo "yml"
        language: system
"#,
        )?;

        let workspace =
            Workspace::discover(DiscoverOptions::default().directory(dir.path().to_path_buf()))?;
        assert_eq!(workspace.projects.len(), 1);

        // Should prefer .yaml file
        assert_eq!(
            workspace.projects[0].config_file().file_name().unwrap(),
            ".pre-commit-config.yaml"
        );

        Ok(())
    }
}
