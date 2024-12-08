use std::collections::{HashMap, HashSet};
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use anyhow::Result;
use clap::ValueEnum;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::zip_eq;
use thiserror::Error;
use tracing::{debug, error};
use url::Url;

use crate::config::{
    self, read_config, read_manifest, ConfigLocalHook, ConfigMetaHook, ConfigRemoteHook,
    ConfigRepo, ConfigWire, Language, ManifestHook, Stage, CONFIG_FILE, MANIFEST_FILE,
};
use crate::fs::{Simplified, CWD};
use crate::languages::DEFAULT_VERSION;
use crate::store::Store;
use crate::warn_user;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Failed to parse URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error(transparent)]
    Config(#[from] config::Error),
    #[error("Hook {hook} in not present in repository {repo}")]
    HookNotFound { hook: String, repo: String },
    #[error(transparent)]
    Store(#[from] Box<crate::store::Error>),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub enum Repo {
    Remote {
        /// Path to the stored repo.
        path: PathBuf,
        url: Url,
        rev: String,
        hooks: Vec<ManifestHook>,
    },
    Local {
        hooks: Vec<ManifestHook>,
    },
    Meta {
        hooks: Vec<ManifestHook>,
    },
}

impl Repo {
    /// Load the remote repo manifest from the path.
    pub fn remote(url: &str, rev: &str, path: &str) -> Result<Self, Error> {
        let url = Url::parse(url)?;

        let path = PathBuf::from(path);
        let manifest = read_manifest(&path.join(MANIFEST_FILE))?;
        let hooks = manifest.hooks;

        Ok(Self::Remote {
            path,
            url,
            rev: rev.to_string(),
            hooks,
        })
    }

    /// Construct a local repo from a list of hooks.
    pub fn local(hooks: Vec<ConfigLocalHook>) -> Self {
        Self::Local { hooks }
    }

    /// Construct a meta repo.
    pub fn meta(hooks: Vec<ConfigMetaHook>) -> Self {
        Self::Meta {
            hooks: hooks.into_iter().map(ManifestHook::from).collect(),
        }
    }

    /// Get a hook by id.
    pub fn get_hook(&self, id: &str) -> Option<&ManifestHook> {
        let hooks = match self {
            Repo::Remote { ref hooks, .. } => hooks,
            Repo::Local { ref hooks } => hooks,
            Repo::Meta { ref hooks } => hooks,
        };
        hooks.iter().find(|hook| hook.id == id)
    }

    /// Get the path to the repo.
    pub fn path(&self) -> &Path {
        match self {
            Repo::Remote { ref path, .. } => path,
            Repo::Local { .. } => &CWD,
            Repo::Meta { .. } => &CWD,
        }
    }
}

impl Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Repo::Remote { url, rev, .. } => write!(f, "{url}@{rev}"),
            Repo::Local { .. } => write!(f, "local"),
            Repo::Meta { .. } => write!(f, "meta"),
        }
    }
}

pub struct Project {
    config_path: PathBuf,
    config: ConfigWire,
    repos: Vec<Rc<Repo>>,
}

impl Project {
    /// Find the configuration file in the given path or the current working directory.
    pub fn find_config_file(config: Option<PathBuf>) -> Result<PathBuf, Error> {
        let file = config.unwrap_or_else(|| CWD.join(CONFIG_FILE));
        if file.try_exists()? {
            return Ok(file);
        }
        let file = file.user_display().to_string();
        Err(Error::Config(config::Error::NotFound(file)))
    }

    /// Initialize a new project from the configuration file or the file in the current working directory.
    pub fn from_config_file(config: Option<PathBuf>) -> Result<Self, Error> {
        let config_path = Self::find_config_file(config)?;
        Self::new(config_path)
    }

    /// Initialize a new project from the configuration file.
    pub fn new(config_path: PathBuf) -> Result<Self, Error> {
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

    pub fn config(&self) -> &ConfigWire {
        &self.config
    }

    pub fn config_file(&self) -> &Path {
        &self.config_path
    }

    async fn init_repos(
        &mut self,
        store: &Store,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<(), Error> {
        let mut tasks = FuturesUnordered::new();
        let mut seen = HashSet::new();
        for repo in &self.config.repos {
            if let ConfigRepo::Remote(repo) = repo {
                if !seen.insert(repo) {
                    continue;
                }
                tasks.push(async move {
                    let progress = reporter
                        .map(|reporter| (reporter, reporter.on_clone_start(&format!("{repo}"))));

                    let path = store.prepare_remote_repo(repo, &[]).await;

                    if let Some((reporter, progress)) = progress {
                        reporter.on_clone_complete(progress);
                    }

                    (repo, path)
                });
            }
        }

        let mut remote_repos = HashMap::new();
        while let Some((repo_config, repo_path)) = tasks.next().await {
            let repo_path = repo_path.map_err(Box::new)?;
            let repo = Rc::new(Repo::remote(
                repo_config.repo.as_str(),
                &repo_config.rev,
                &repo_path.to_string_lossy(),
            )?);
            remote_repos.insert(repo_config, repo.clone());
        }

        let mut repos = Vec::with_capacity(self.config.repos.len());
        for repo in &self.config.repos {
            match repo {
                ConfigRepo::Remote(repo_config) => {
                    let repo = remote_repos.get(repo_config).expect("repo not found");
                    repos.push(repo.clone());
                }
                ConfigRepo::Local(repo) => {
                    let repo = Repo::local(repo.hooks.clone());
                    repos.push(Rc::new(repo));
                }
                ConfigRepo::Meta(repo) => {
                    let repo = Repo::meta(repo.hooks.clone());
                    repos.push(Rc::new(repo));
                }
            }
        }

        self.repos = repos;

        Ok(())
    }

    /// Load and prepare hooks for the project.
    pub async fn init_hooks(
        &mut self,
        store: &Store,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<Vec<Hook>, Error> {
        self.init_repos(store, reporter).await?;

        let mut hooks = Vec::new();

        for (repo_config, repo) in zip_eq(self.config.repos.iter(), self.repos.iter()) {
            match repo_config {
                ConfigRepo::Remote(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        // Check hook id is valid.
                        let Some(hook) = repo.get_hook(&hook_config.id) else {
                            return Err(Error::HookNotFound {
                                hook: hook_config.id.clone(),
                                repo: repo.to_string(),
                            });
                        };

                        let repo = Rc::clone(repo);
                        let mut builder = HookBuilder::new(repo, hook.clone());
                        builder.update(hook_config);
                        builder.combine(&self.config);
                        let mut hook = builder.build();

                        if hook.additional_dependencies.is_empty() {
                            // Use the shared repo environment.
                            let path = hook.repo.path().to_path_buf();
                            hook = hook.with_path(path);
                        } else {
                            // Prepare hooks with `additional_dependencies` (they need separate environments).
                            let path = store
                                .prepare_remote_repo(repo_config, &hook.additional_dependencies)
                                .await
                                .map_err(Box::new)?;

                            hook = hook.with_path(path);
                        }

                        hooks.push(hook);
                    }
                }
                ConfigRepo::Local(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Rc::clone(repo);
                        let mut builder = HookBuilder::new(repo, hook_config.clone());
                        builder.combine(&self.config);
                        let mut hook = builder.build();

                        // If the hook doesn't need an environment, don't do any preparation.
                        if hook.language.environment_dir().is_some() {
                            let path = store
                                .prepare_local_repo(&hook, &hook.additional_dependencies)
                                .map_err(Box::new)?;

                            hook = hook.with_path(path);
                        } else {
                            // Use the shared repo environment.
                            let path = hook.repo.path().to_path_buf();
                            hook = hook.with_path(path);
                        }
                        hooks.push(hook);
                    }
                }
                ConfigRepo::Meta(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Rc::clone(repo);
                        let hook_config = ManifestHook::from(hook_config.clone());
                        let mut builder = HookBuilder::new(repo, hook_config);
                        builder.combine(&self.config);
                        let mut hook = builder.build();

                        let path = hook.repo.path().to_path_buf();
                        hook = hook.with_path(path);
                        hooks.push(hook);
                    }
                }
            }
        }

        reporter.map(HookInitReporter::on_complete);

        Ok(hooks)
    }
}

pub trait HookInitReporter {
    fn on_clone_start(&self, repo: &str) -> usize;
    fn on_clone_complete(&self, id: usize);
    fn on_complete(&self);
}

struct HookBuilder {
    repo: Rc<Repo>,
    config: ManifestHook,
}

impl HookBuilder {
    fn new(repo: Rc<Repo>, config: ManifestHook) -> Self {
        Self { repo, config }
    }

    /// Update the hook from the project level hook configuration.
    fn update(&mut self, config: &ConfigRemoteHook) -> &mut Self {
        if let Some(name) = &config.name {
            self.config.name.clone_from(name);
        }
        if let Some(entry) = &config.entry {
            self.config.entry.clone_from(entry);
        }
        if let Some(language) = &config.language {
            self.config.language.clone_from(language);
        }

        self.config.options.update(&config.options);

        self
    }

    /// Combine the hook configuration with the project level hook configuration.
    fn combine(&mut self, config: &ConfigWire) {
        let options = &mut self.config.options;
        let language = self.config.language;
        if options.language_version.is_none() {
            options.language_version = config
                .default_language_version
                .as_ref()
                .and_then(|v| v.get(&language).cloned());
        }
        if options.language_version.is_none() {
            options.language_version = Some(language.default_version().to_string());
        }

        if options.stages.is_none() {
            options.stages.clone_from(&config.default_stages);
        }
    }

    /// Fill in the default values for the hook configuration.
    fn fill_in_defaults(&mut self) {
        let options = &mut self.config.options;
        options
            .language_version
            .get_or_insert(DEFAULT_VERSION.to_string());
        options.alias.get_or_insert(String::new());
        options.args.get_or_insert(Vec::new());
        options.types.get_or_insert(vec!["file".to_string()]);
        options.types_or.get_or_insert(Vec::new());
        options.exclude_types.get_or_insert(Vec::new());
        options.always_run.get_or_insert(false);
        options.fail_fast.get_or_insert(false);
        options.pass_filenames.get_or_insert(true);
        options.require_serial.get_or_insert(false);
        options.verbose.get_or_insert(false);
        options
            .stages
            .get_or_insert(Stage::value_variants().to_vec());
        options.additional_dependencies.get_or_insert(Vec::new());
    }

    /// Check the hook configuration.
    fn check(&self) {
        let language = self.config.language;
        if language.environment_dir().is_none() {
            if self.config.options.language_version != Some(DEFAULT_VERSION.to_string()) {
                warn_user!(
                    "Language {} does not need environment, but language_version is set",
                    language
                );
            }

            if self.config.options.additional_dependencies.is_some() {
                warn_user!(
                    "Language {} does not need environment, but additional_dependencies is set",
                    language
                );
            }
        }
    }

    /// Build the hook.
    fn build(mut self) -> Hook {
        self.check();
        self.fill_in_defaults();

        let options = self.config.options;
        Hook {
            repo: self.repo,
            path: None,
            id: self.config.id,
            name: self.config.name,
            entry: self.config.entry,
            language: self.config.language,
            alias: options.alias.expect("alias not set"),
            files: options.files,
            exclude: options.exclude,
            types: options.types.expect("types not set"),
            types_or: options.types_or.expect("types_or not set"),
            exclude_types: options.exclude_types.expect("exclude_types not set"),
            additional_dependencies: options
                .additional_dependencies
                .expect("additional_dependencies should not be None"),
            args: options.args.expect("args not set"),
            always_run: options.always_run.expect("always_run not set"),
            fail_fast: options.fail_fast.expect("fail_fast not set"),
            pass_filenames: options.pass_filenames.expect("pass_filenames not set"),
            description: options.description,
            language_version: options.language_version.expect("language_version not set"),
            log_file: options.log_file,
            require_serial: options.require_serial.expect("require_serial not set"),
            stages: options.stages.expect("stages not set"),
            verbose: options.verbose.expect("verbose not set"),
            minimum_pre_commit_version: options.minimum_pre_commit_version,
        }
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub struct Hook {
    repo: Rc<Repo>,
    path: Option<PathBuf>,

    pub id: String,
    pub name: String,
    pub entry: String,
    pub language: Language,
    pub alias: String,
    pub files: Option<String>,
    pub exclude: Option<String>,
    pub types: Vec<String>,
    pub types_or: Vec<String>,
    pub exclude_types: Vec<String>,
    pub additional_dependencies: Vec<String>,
    pub args: Vec<String>,
    pub always_run: bool,
    pub fail_fast: bool,
    pub pass_filenames: bool,
    pub description: Option<String>,
    pub language_version: String,
    pub log_file: Option<String>,
    pub require_serial: bool,
    pub stages: Vec<Stage>,
    pub verbose: bool,
    pub minimum_pre_commit_version: Option<String>,
}

impl Display for Hook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            if let Some(ref path) = self.path {
                write!(
                    f,
                    "{} ({} at {})",
                    self.id,
                    self.repo,
                    path.to_string_lossy()
                )
            } else {
                write!(f, "{} ({})", self.id, self.repo)
            }
        } else {
            write!(f, "{}", self.id)
        }
    }
}

impl Hook {
    pub fn with_path(mut self, path: PathBuf) -> Self {
        self.path = Some(path);
        self
    }

    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// Get the working directory for the hook.
    pub fn path(&self) -> &Path {
        self.path.as_deref().unwrap_or_else(|| self.repo.path())
    }

    /// Get the environment directory that the hook will be installed to.
    pub fn environment_dir(&self) -> Option<PathBuf> {
        let env_dir = self.language.environment_dir()?;
        Some(
            self.path()
                .join(format!("{}-{}", env_dir, &self.language_version)),
        )
    }

    pub fn install_key(&self) -> String {
        format!(
            "{}-{}-{}-{}",
            self.repo,
            self.language,
            self.language_version,
            self.additional_dependencies.join(",")
        )
    }

    // TODO: health check
    /// Check if the hook is installed in the environment.
    pub fn installed(&self) -> bool {
        let Some(env) = self.environment_dir() else {
            return true;
        };

        let state_file_v2 = env.join(".install_state_v2");
        state_file_v2.exists()
        // Drop support for state file v1.
    }

    /// Write a state file to mark the hook as installed.
    pub fn mark_installed(&self) -> Result<(), Error> {
        let env = self.environment_dir().unwrap();
        let state_file_v2 = env.join(".install_state_v2");
        fs_err::OpenOptions::new()
            .write(true)
            .create(true)
            .open(&state_file_v2)?;
        Ok(())
    }
}
