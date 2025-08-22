use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use futures::StreamExt;
use itertools::zip_eq;
use rustc_hash::{FxHashMap, FxHashSet};
use thiserror::Error;
use tracing::{debug, error};

use crate::config::{self, CONFIG_FILE, Config, ManifestHook, read_config};
use crate::hook::{self, Hook, HookBuilder, Repo};
use crate::store;
use crate::store::Store;

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

pub(crate) struct Project {
    config_path: PathBuf,
    config: Config,
    repos: Vec<Arc<Repo>>,
}

impl Project {
    /// Initialize a new project from the configuration file or the file in the current working directory.
    pub(crate) fn from_config_file(config_path: PathBuf) -> Result<Self, Error> {
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
    pub(crate) fn from_directory(path: &Path) -> Result<Self, Error> {
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
    ) -> Result<Self, Error> {
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

                        let hook = builder.build().await?;
                        hooks.push(hook);
                    }
                }
                config::Repo::Local(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Arc::clone(repo);
                        let mut builder = HookBuilder::new(repo, hook_config.clone(), hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build().await?;
                        hooks.push(hook);
                    }
                }
                config::Repo::Meta(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Arc::clone(repo);
                        let hook_config = ManifestHook::from(hook_config.clone());
                        let mut builder = HookBuilder::new(repo, hook_config, hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build().await?;
                        hooks.push(hook);
                    }
                }
            }
        }

        reporter.map(HookInitReporter::on_complete);

        Ok(hooks)
    }
}
