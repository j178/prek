use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use futures::StreamExt;
use itertools::zip_eq;
use rustc_hash::FxHashMap;
use thiserror::Error;
use tracing::{debug, error};

use crate::config::{self, ALTER_CONFIG_FILE, CONFIG_FILE, Config, ManifestHook, read_config};
use crate::fs::{CWD, Simplified};
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

pub(crate) struct Project {
    config_path: PathBuf,
    config: Config,
    repos: Vec<Arc<Repo>>,
}

impl Project {
    /// Find the configuration file in the given path or the current working directory.
    pub(crate) fn find_config_file(config: Option<PathBuf>) -> Result<PathBuf, Error> {
        if let Some(config) = config {
            if config.exists() {
                return Ok(config);
            }
            return Err(Error::InvalidConfig(config::Error::NotFound(
                config.user_display().to_string(),
            )));
        }

        let main = CWD.join(CONFIG_FILE);
        let alternate = CWD.join(ALTER_CONFIG_FILE);
        if main.exists() && alternate.exists() {
            warn_user!(
                "Both {main} and {alternate} exist, using {main}",
                main = main.display(),
                alternate = alternate.display()
            );
        }
        if main.exists() {
            return Ok(main);
        }
        if alternate.exists() {
            return Ok(alternate);
        }

        Err(Error::InvalidConfig(config::Error::NotFound(
            CONFIG_FILE.into(),
        )))
    }

    /// Initialize a new project from the configuration file or the file in the current working directory.
    pub(crate) fn from_config_file(config: Option<PathBuf>) -> Result<Self, Error> {
        let config_path = Self::find_config_file(config)?;
        Self::new(config_path)
    }

    /// Initialize a new project from the configuration file.
    pub(crate) fn new(config_path: PathBuf) -> Result<Self, Error> {
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
        let mut seen = HashSet::new();

        // Prepare remote repos in parallel.
        let remotes_iter = self.config.repos.iter().filter_map(|repo| match repo {
            // Deduplicate remote repos.
            config::Repo::Remote(repo) if seen.insert(repo) => Some(repo),
            _ => None,
        });
        let mut tasks = futures::stream::iter(remotes_iter)
            .map(async |repo_config| {
                let remote_repos = remote_repos.clone();

                let progress = reporter
                    .map(|reporter| (reporter, reporter.on_clone_start(&format!("{repo_config}"))));

                let path = store
                    .clone_repo(repo_config)
                    .await
                    .map_err(|e| Error::Store {
                        repo: format!("{}", repo_config.repo),
                        error: Box::new(e),
                    })?;

                if let Some((reporter, progress)) = progress {
                    reporter.on_clone_complete(progress);
                }

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
