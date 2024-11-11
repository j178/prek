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
    self, read_config, read_manifest, ConfigLocalHook, ConfigRemoteHook, ConfigRepo, ConfigWire,
    ManifestHook, Stage, CONFIG_FILE, MANIFEST_FILE,
};
use crate::fs::{Simplified, CWD};
use crate::languages::{Language, DEFAULT_VERSION};
use crate::printer::Printer;
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
    Meta,
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

    pub fn meta() -> Result<Self, Error> {
        todo!()
    }

    /// Get a hook by id.
    pub fn get_hook(&self, id: &str) -> Option<&ManifestHook> {
        let hooks = match self {
            Repo::Remote { ref hooks, .. } => hooks,
            Repo::Local { ref hooks } => hooks,
            Repo::Meta => return None,
        };
        hooks.iter().find(|hook| hook.id == id)
    }

    pub fn path(&self) -> &Path {
        match self {
            Repo::Remote { ref path, .. } => path,
            Repo::Local { .. } => &CWD,
            Repo::Meta => todo!(),
        }
    }
}

impl Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Repo::Remote { url, rev, .. } => write!(f, "{url}@{rev}"),
            Repo::Local { .. } => write!(f, "local"),
            Repo::Meta => write!(f, "meta"),
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

    async fn init_repos(&mut self, store: &Store, printer: Printer) -> Result<(), Error> {
        let mut repos = Vec::with_capacity(self.config.repos.len());

        // TODO: progress bar
        let mut tasks = FuturesUnordered::new();
        for (idx, repo) in self.config.repos.iter().enumerate() {
            match repo {
                ConfigRepo::Remote(repo) => {
                    tasks.push(async move {
                        let path = store.prepare_remote_repo(repo, &[], printer).await;
                        (idx, path)
                    });
                }
                ConfigRepo::Local(repo) => {
                    let repo = Repo::local(repo.hooks.clone());
                    repos.push((idx, Rc::new(repo)));
                }
                ConfigRepo::Meta(_) => {
                    todo!()
                }
            }
        }

        while let Some((idx, repo_path)) = tasks.next().await {
            let repo_path = repo_path.map_err(Box::new)?;
            let ConfigRepo::Remote(repo_config) = &self.config.repos[idx] else {
                unreachable!();
            };
            let repo = Repo::remote(
                repo_config.repo.as_str(),
                &repo_config.rev,
                &repo_path.to_string_lossy(),
            )?;
            repos.push((idx, Rc::new(repo)));
        }

        repos.sort_unstable_by_key(|(idx, _)| *idx);
        self.repos = repos.into_iter().map(|(_, repo)| repo).collect();

        Ok(())
    }

    /// Load and prepare hooks for the project.
    pub async fn init_hooks(
        &mut self,
        store: &Store,
        printer: Printer,
    ) -> Result<Vec<Hook>, Error> {
        self.init_repos(store, printer).await?;

        let mut hooks = Vec::new();

        // TODO: progress bar
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
                                .prepare_remote_repo(
                                    repo_config,
                                    &hook.additional_dependencies,
                                    printer,
                                )
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
                                .prepare_local_repo(&hook, &hook.additional_dependencies, printer)
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
                ConfigRepo::Meta(_) => {
                    todo!()
                }
            }
        }

        Ok(hooks)
    }
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
        macro_rules! update_if_some {
            ($($field:ident),* $(,)?) => {
                $(
                if config.$field.is_some() {
                    self.config.$field.clone_from(&config.$field);
                }
                )*
            };
        }
        update_if_some!(
            alias,
            files,
            exclude,
            types,
            types_or,
            exclude_types,
            additional_dependencies,
            args,
            always_run,
            fail_fast,
            pass_filenames,
            description,
            language_version,
            log_file,
            require_serial,
            stages,
            verbose,
            minimum_pre_commit_version,
        );

        if let Some(name) = &config.name {
            self.config.name.clone_from(name);
        }
        if let Some(entry) = &config.entry {
            self.config.entry.clone_from(entry);
        }
        if let Some(language) = &config.language {
            self.config.language.clone_from(language);
        }

        self
    }

    /// Combine the hook configuration with the project level hook configuration.
    fn combine(&mut self, config: &ConfigWire) {
        let language = self.config.language;
        if self.config.language_version.is_none() {
            self.config.language_version = config
                .default_language_version
                .as_ref()
                .and_then(|v| v.get(&language).cloned());
        }
        if self.config.language_version.is_none() {
            self.config.language_version =
                Some(Language::from(language).default_version().to_string());
        }

        if self.config.stages.is_none() {
            self.config.stages.clone_from(&config.default_stages);
        }
    }

    /// Fill in the default values for the hook configuration.
    fn fill_in_defaults(&mut self) {
        self.config
            .language_version
            .get_or_insert(DEFAULT_VERSION.to_string());
        self.config.alias.get_or_insert(String::new());
        self.config.args.get_or_insert(Vec::new());
        self.config.types.get_or_insert(vec!["file".to_string()]);
        self.config.types_or.get_or_insert(Vec::new());
        self.config.exclude_types.get_or_insert(Vec::new());
        self.config.always_run.get_or_insert(false);
        self.config.fail_fast.get_or_insert(false);
        self.config.pass_filenames.get_or_insert(true);
        self.config.require_serial.get_or_insert(false);
        self.config.verbose.get_or_insert(false);
        self.config
            .stages
            .get_or_insert(Stage::value_variants().to_vec());
        self.config
            .additional_dependencies
            .get_or_insert(Vec::new());
    }

    /// Check the hook configuration.
    fn check(&self) {
        let language = Language::from(self.config.language);
        // TODO: check ENVIRONMENT_DIR with language_version and additional_dependencies
        if language.environment_dir().is_none() {
            if self.config.language_version != Some(DEFAULT_VERSION.to_string()) {
                warn_user!(
                    "Language {} does not need environment, but language_version is set",
                    language
                );
            }

            if self.config.additional_dependencies.is_some() {
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

        Hook {
            repo: self.repo,
            path: None,
            id: self.config.id,
            name: self.config.name,
            entry: self.config.entry,
            language: self.config.language.into(),
            alias: self.config.alias.expect("alias not set"),
            files: self.config.files,
            exclude: self.config.exclude,
            types: self.config.types.expect("types not set"),
            types_or: self.config.types_or.expect("types_or not set"),
            exclude_types: self.config.exclude_types.expect("exclude_types not set"),
            additional_dependencies: self
                .config
                .additional_dependencies
                .expect("additional_dependencies should not be None"),
            args: self.config.args.expect("args not set"),
            always_run: self.config.always_run.expect("always_run not set"),
            fail_fast: self.config.fail_fast.expect("fail_fast not set"),
            pass_filenames: self.config.pass_filenames.expect("pass_filenames not set"),
            description: self.config.description,
            language_version: self
                .config
                .language_version
                .expect("language_version not set"),
            log_file: self.config.log_file,
            require_serial: self.config.require_serial.expect("require_serial not set"),
            stages: self.config.stages.expect("stages not set"),
            verbose: self.config.verbose.expect("verbose not set"),
            minimum_pre_commit_version: self.config.minimum_pre_commit_version,
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
