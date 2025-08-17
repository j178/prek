use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use futures::StreamExt;
use ignore::WalkState;
use itertools::zip_eq;
use rustc_hash::{FxHashMap, FxHashSet};
use thiserror::Error;
use tracing::{debug, error};

use crate::config::{self, ALTER_CONFIG_FILE, CONFIG_FILE, Config, ManifestHook, read_config};
use crate::fs::Simplified;
use crate::hook::{self, Hook, HookBuilder, Repo};
use crate::store::Store;
use crate::{store, warn_user};

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error(transparent)]
    InvalidConfig(#[from] config::Error),

    #[error(transparent)]
    Hook(#[from] hook::Error),

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
    config: Config,
    repos: Vec<Arc<Repo>>,
}

impl Project {
    /// Initialize a new project from the configuration file or the file in the current working directory.
    pub(crate) fn from_config_file(config_path: PathBuf) -> Result<Self, config::Error> {
        debug!(
            path = %config_path.display(),
            "Loading project configuration"
        );
        let config = read_config(&config_path)?;
        let size = config.repos.len();
        Ok(Self {
            config,
            config_path,
            repos: Vec::with_capacity(size),
        })
    }

    /// Find the configuration file in the given path.
    pub(crate) fn from_directory(path: &Path) -> Result<Self, config::Error> {
        let main = path.join(CONFIG_FILE);
        let alternate = path.join(ALTER_CONFIG_FILE);
        if main.exists() && alternate.exists() {
            warn_user!(
                "Both `{main}` and `{alternate}` exist, using `{main}`",
                main = main.user_display().cyan(),
                alternate = alternate.user_display().cyan()
            );
        }
        if main.exists() {
            return Self::from_config_file(main);
        }
        if alternate.exists() {
            return Self::from_config_file(alternate);
        }

        Err(config::Error::NotFound(main.user_display().to_string()))
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

    pub(crate) fn config(&self) -> &Config {
        &self.config
    }

    pub(crate) fn config_file(&self) -> &Path {
        &self.config_path
    }

    async fn init_repos(
        &mut self,
        store: &Store,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<(), Error> {
        let remote_repos = Rc::new(Mutex::new(FxHashMap::default()));
        #[allow(clippy::mutable_key_type)]
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
                    let remote_repos = remote_repos.clone();

                    let path = store.clone_repo(repo_config, reporter).await.map_err(|e| {
                        Error::Store {
                            repo: format!("{}", repo_config.repo),
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

        let mut repos = Vec::with_capacity(self.config.repos.len());
        let remote_repos = remote_repos.lock().unwrap();
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
    pub(crate) async fn init_hooks(
        &mut self,
        store: &Store,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<Vec<Hook>, Error> {
        self.init_repos(store, reporter).await?;

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
                        let mut builder = HookBuilder::new(repo, hook.clone(), hooks.len());
                        builder.update(hook_config);
                        builder.combine(&self.config);

                        let hook = builder.build()?;
                        hooks.push(hook);
                    }
                }
                config::Repo::Local(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Arc::clone(repo);
                        let mut builder = HookBuilder::new(repo, hook_config.clone(), hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build()?;
                        hooks.push(hook);
                    }
                }
                config::Repo::Meta(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Arc::clone(repo);
                        let hook_config = ManifestHook::from(hook_config.clone());
                        let mut builder = HookBuilder::new(repo, hook_config, hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build()?;
                        hooks.push(hook);
                    }
                }
            }
        }

        reporter.map(HookInitReporter::on_complete);

        Ok(hooks)
    }
}

pub(crate) struct Workspace {
    projects: Vec<Project>,
}

impl Workspace {
    pub(crate) fn discover(path: &Path) -> Result<Self, config::Error> {
        let projects = Mutex::new(Ok(Vec::new()));

        ignore::WalkBuilder::new(path)
            .follow_links(false)
            .build_parallel()
            .run(|| {
                Box::new(|result| {
                    if let Ok(entry) = result {
                        if entry.file_type().is_some_and(|t| t.is_dir()) {
                            match Project::from_directory(entry.path()) {
                                Ok(project) => {
                                    debug!(
                                        path = %project.config_file().display(),
                                        "Found project configuration"
                                    );
                                    projects.lock().unwrap().as_mut().unwrap().push(project);
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

        let projects = projects.into_inner().unwrap()?;
        Ok(Self { projects })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn test_workspace_discovery_empty_directory() -> Result<()> {
        let dir = tempfile::tempdir()?;

        let workspace = Workspace::discover(dir.path())?;
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

        let workspace = Workspace::discover(dir.path())?;
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

        let workspace = Workspace::discover(dir.path())?;
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

        let workspace = Workspace::discover(dir.path())?;
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

        let workspace = Workspace::discover(dir.path())?;
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
        let result = Workspace::discover(dir.path());
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

        let workspace = Workspace::discover(dir.path())?;
        assert_eq!(workspace.projects.len(), 1);

        // Should prefer .yaml file
        assert_eq!(
            workspace.projects[0].config_file().file_name().unwrap(),
            ".pre-commit-config.yaml"
        );

        Ok(())
    }
}
