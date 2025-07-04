use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fmt::{Display, Formatter};
use std::hash::{Hash, Hasher};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Mutex;

use anyhow::Result;
use clap::ValueEnum;
use futures::StreamExt;
use itertools::zip_eq;
use seahash::SeaHasher;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tracing::{debug, error};
use url::Url;

use crate::config::{
    self, ALTER_CONFIG_FILE, CONFIG_FILE, Config, Language, LanguageVersion, LocalHook,
    MANIFEST_FILE, ManifestHook, MetaHook, RemoteHook, Stage, read_config, read_manifest,
};
use crate::fs::{CWD, Simplified};
use crate::store::{Store, to_hex};
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
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

#[derive(Debug, Clone)]
pub enum Repo {
    Remote {
        /// Path to the cloned repo.
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
    pub fn remote(url: Url, rev: String, path: PathBuf) -> Result<Self, Error> {
        let manifest = read_manifest(&path.join(MANIFEST_FILE))?;
        let hooks = manifest.hooks;

        Ok(Self::Remote {
            path,
            url,
            rev,
            hooks,
        })
    }

    /// Construct a local repo from a list of hooks.
    pub fn local(hooks: Vec<LocalHook>) -> Self {
        Self::Local { hooks }
    }

    /// Construct a meta repo.
    pub fn meta(hooks: Vec<MetaHook>) -> Self {
        Self::Meta {
            hooks: hooks.into_iter().map(ManifestHook::from).collect(),
        }
    }

    /// Get the path to the cloned repo if it is a remote repo.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Repo::Remote { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Get a hook by id.
    pub fn get_hook(&self, id: &str) -> Option<&ManifestHook> {
        let hooks = match self {
            Repo::Remote { hooks, .. } => hooks,
            Repo::Local { hooks } => hooks,
            Repo::Meta { hooks } => hooks,
        };
        hooks.iter().find(|hook| hook.id == id)
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
    config: Config,
    repos: Vec<Rc<Repo>>,
}

impl Project {
    /// Find the configuration file in the given path or the current working directory.
    pub fn find_config_file(config: Option<PathBuf>) -> Result<PathBuf, Error> {
        if let Some(config) = config {
            if config.try_exists()? {
                return Ok(config);
            }
            return Err(Error::Config(config::Error::NotFound(
                config.user_display().to_string(),
            )));
        }

        let main = CWD.join(CONFIG_FILE);
        let alternate = CWD.join(ALTER_CONFIG_FILE);
        if main.try_exists()? && alternate.try_exists()? {
            warn_user!(
                "Both {main} and {alternate} exist, using {main}",
                main = main.display(),
                alternate = alternate.display()
            );
        }
        if main.try_exists()? {
            return Ok(main);
        }
        if alternate.try_exists()? {
            return Ok(alternate);
        }

        Err(Error::Config(config::Error::NotFound(CONFIG_FILE.into())))
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

    pub fn config(&self) -> &Config {
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
        let remote_repos = Rc::new(Mutex::new(HashMap::new()));
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

                let path = store.clone_repo(repo_config).await.map_err(Box::new)?;

                if let Some((reporter, progress)) = progress {
                    reporter.on_clone_complete(progress);
                }

                let repo = Rc::new(Repo::remote(
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
                    repos.push(Rc::new(repo));
                }
                config::Repo::Meta(repo) => {
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
                config::Repo::Remote(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        // Check hook id is valid.
                        let Some(hook) = repo.get_hook(&hook_config.id) else {
                            return Err(Error::HookNotFound {
                                hook: hook_config.id.clone(),
                                repo: repo.to_string(),
                            });
                        };

                        let repo = Rc::clone(repo);
                        let mut builder = HookBuilder::new(repo, hook.clone(), hooks.len());
                        builder.update(hook_config);
                        builder.combine(&self.config);

                        let hook = builder.build();
                        hooks.push(hook);
                    }
                }
                config::Repo::Local(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Rc::clone(repo);
                        let mut builder = HookBuilder::new(repo, hook_config.clone(), hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build();
                        hooks.push(hook);
                    }
                }
                config::Repo::Meta(repo_config) => {
                    for hook_config in &repo_config.hooks {
                        let repo = Rc::clone(repo);
                        let hook_config = ManifestHook::from(hook_config.clone());
                        let mut builder = HookBuilder::new(repo, hook_config, hooks.len());
                        builder.combine(&self.config);

                        let hook = builder.build();
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
    idx: usize,
}

impl HookBuilder {
    fn new(repo: Rc<Repo>, config: ManifestHook, idx: usize) -> Self {
        Self { repo, config, idx }
    }

    /// Update the hook from the project level hook configuration.
    fn update(&mut self, config: &RemoteHook) -> &mut Self {
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

    /// Combine the hook configuration with the project level configuration.
    fn combine(&mut self, config: &Config) {
        let options = &mut self.config.options;
        let language = self.config.language;
        if options.language_version.is_none() {
            options.language_version = config
                .default_language_version
                .as_ref()
                .and_then(|v| v.get(&language).cloned());
        }

        if options.stages.is_none() {
            options.stages.clone_from(&config.default_stages);
        }
    }

    /// Fill in the default values for the hook configuration.
    fn fill_in_defaults(&mut self) {
        let options = &mut self.config.options;
        options.language_version.get_or_insert_default();
        options.alias.get_or_insert_default();
        options.args.get_or_insert_default();
        options.types.get_or_insert(vec!["file".to_string()]);
        options.types_or.get_or_insert_default();
        options.exclude_types.get_or_insert_default();
        options.always_run.get_or_insert(false);
        options.fail_fast.get_or_insert(false);
        options.pass_filenames.get_or_insert(true);
        options.require_serial.get_or_insert(false);
        options.verbose.get_or_insert(false);
        options
            .stages
            .get_or_insert(Stage::value_variants().to_vec());
        options.additional_dependencies.get_or_insert_default();
    }

    /// Check the hook configuration.
    fn check(&self) {
        let language = self.config.language;
        let options = &self.config.options;
        if !language.supports_dependency() {
            if options.additional_dependencies.is_some() {
                warn_user!(
                    "Language {} does not need environment, but additional_dependencies is set",
                    language
                );
            }
        }
        if options
            .language_version
            .as_ref()
            .and_then(|v| v.request.as_ref())
            .is_some()
        {
            if !language.supports_dependency() {
                warn_user!(
                    "Language {} does not need environment, but language_version is set",
                    language
                );
            } else if !language.supports_language_version() {
                warn_user!(
                    "Language {} does not support specifying version, but language_version is set",
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
            idx: self.idx,
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

    /// The index of the hook defined in the configuration file.
    pub idx: usize,
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
    pub language_version: LanguageVersion,
    pub log_file: Option<String>,
    pub require_serial: bool,
    pub stages: Vec<Stage>,
    pub verbose: bool,
    pub minimum_pre_commit_version: Option<String>,
}

impl Display for Hook {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        if f.alternate() {
            write!(f, "{}:{}", self.repo, self.id)
        } else {
            write!(f, "{}", self.id)
        }
    }
}

impl Hook {
    pub fn repo(&self) -> &Repo {
        &self.repo
    }

    /// Get the path to the repository that contains the hook.
    pub fn repo_path(&self) -> Option<&Path> {
        self.repo.path()
    }

    pub fn is_local(&self) -> bool {
        matches!(&*self.repo, Repo::Local { .. })
    }

    pub fn is_remote(&self) -> bool {
        matches!(&*self.repo, Repo::Remote { .. })
    }

    pub fn is_meta(&self) -> bool {
        matches!(&*self.repo, Repo::Meta { .. })
    }

    pub fn dependencies(&self) -> Cow<'_, [String]> {
        // For remote hooks, itself is an implicit dependency of the hook.
        if self.is_remote() {
            let mut deps = Vec::with_capacity(1 + self.additional_dependencies.len());
            deps.push(self.repo.to_string());
            deps.extend(self.additional_dependencies.iter().map(ToString::to_string));
            Cow::Owned(deps)
        } else {
            Cow::Borrowed(&self.additional_dependencies)
        }
    }
}

#[derive(Debug, Clone)]
pub enum ResolvedHook {
    Installed {
        hook: Hook,
        info: InstallInfo,
    },
    NotInstalled {
        hook: Hook,
        info: InstallInfo,
        /// Additional resolved toolchain information, like the path to Python executable.
        toolchain: PathBuf,
    },
    NoNeedInstall(Hook),
}

impl Deref for ResolvedHook {
    type Target = Hook;

    fn deref(&self) -> &Self::Target {
        match self {
            ResolvedHook::Installed { hook, .. } => hook,
            ResolvedHook::NotInstalled { hook, .. } => hook,
            ResolvedHook::NoNeedInstall(hook) => hook,
        }
    }
}

impl Display for ResolvedHook {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // TODO: add more information
        self.deref().fmt(f)
    }
}

impl Hash for ResolvedHook {
    fn hash<H: Hasher>(&self, state: &mut H) {
        match self {
            ResolvedHook::Installed { info, .. } => {
                info.hash(state);
            }
            ResolvedHook::NotInstalled { info, .. } => {
                info.hash(state);
            }
            ResolvedHook::NoNeedInstall(hook) => {
                hook.to_string().hash(state);
            }
        }
    }
}

impl ResolvedHook {
    pub fn env_path(&self) -> Option<&Path> {
        match self {
            ResolvedHook::Installed { info, .. } => Some(&info.env_path),
            ResolvedHook::NotInstalled { info, .. } => Some(&info.env_path),
            ResolvedHook::NoNeedInstall(_) => None,
        }
    }

    /// Check if the hook is installed in the environment.
    pub fn installed(&self) -> bool {
        !matches!(self, ResolvedHook::NotInstalled { .. })
    }

    /// Mark the hook as installed in the environment.
    pub async fn mark_as_installed(&self, _store: &Store) -> Result<(), Error> {
        let Self::NotInstalled { info, .. } = self else {
            return Ok(());
        };

        let content = serde_json::to_string_pretty(info)?;
        fs_err::tokio::write(info.env_path.join(".prefligit-hook.json"), content).await?;
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct InstallInfo {
    pub language: Language,
    pub language_version: semver::Version,
    pub dependencies: Vec<String>,
    pub env_path: PathBuf,
}

impl Hash for InstallInfo {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.language.hash(state);
        self.language_version.hash(state);
        self.dependencies.hash(state);
    }
}

impl InstallInfo {
    pub fn new(
        language: Language,
        language_version: semver::Version,
        dependencies: Vec<String>,
        store: &Store,
    ) -> Self {
        // Calculate the hook directory.
        let mut hasher = SeaHasher::new();
        language.hash(&mut hasher);
        language_version.hash(&mut hasher);
        dependencies.hash(&mut hasher);
        let hash = to_hex(hasher.finish());

        Self {
            language,
            language_version,
            dependencies,
            env_path: store.hooks_dir().join(hash),
        }
    }

    pub fn matches(&self, hook: &Hook) -> bool {
        self.language == hook.language
            && hook.language_version.matches(&self.language_version)
            // TODO: should we compare ignore order?
            && self.dependencies.as_slice() == &*hook.dependencies()
    }
}
