use std::borrow::Cow;
use std::ffi::OsStr;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use anyhow::{Context, Result};
use clap::ValueEnum;
use prek_consts::MANIFEST_FILE;
use rustc_hash::{FxBuildHasher, FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use thiserror::Error;
use tracing::trace;

use crate::config::{
    self, BuiltinHook, Config, FilePattern, HookOptions, Language, LocalHook, ManifestHook,
    MetaHook, RemoteHook, Stage, read_manifest,
};
use crate::languages::version::LanguageRequest;
use crate::languages::{extract_metadata_from_entry, resolve_command};
use crate::store::Store;
use crate::workspace::Project;

#[derive(Error, Debug)]
pub(crate) enum Error {
    #[error(transparent)]
    Config(#[from] config::Error),

    #[error("Invalid hook `{hook}`")]
    Hook {
        hook: String,
        #[source]
        error: anyhow::Error,
    },

    #[error("Failed to read manifest of `{repo}`")]
    Manifest {
        repo: String,
        #[source]
        error: config::Error,
    },

    #[error("Failed to create directory for hook environment")]
    TmpDir(#[from] std::io::Error),
}

/// A hook specification that all hook types can be converted into.
#[derive(Debug, Clone)]
pub(crate) struct HookSpec {
    pub id: String,
    pub name: String,
    pub entry: String,
    pub language: Language,
    pub priority: Option<u32>,
    pub options: HookOptions,
}

impl HookSpec {
    pub(crate) fn apply_remote_hook_overrides(&mut self, config: &RemoteHook) {
        if let Some(name) = &config.name {
            self.name.clone_from(name);
        }
        if let Some(entry) = &config.entry {
            self.entry.clone_from(entry);
        }
        if let Some(language) = &config.language {
            self.language.clone_from(language);
        }
        if let Some(priority) = config.priority {
            self.priority = Some(priority);
        }

        self.options.update(&config.options);
    }

    pub(crate) fn apply_project_defaults(&mut self, config: &Config) {
        let language = self.language;
        if self.options.language_version.is_none() {
            self.options.language_version = config
                .default_language_version
                .as_ref()
                .and_then(|v| v.get(&language).cloned());
        }

        if self.options.stages.is_none() {
            self.options.stages.clone_from(&config.default_stages);
        }
    }
}

impl From<ManifestHook> for HookSpec {
    fn from(hook: ManifestHook) -> Self {
        Self {
            id: hook.id,
            name: hook.name,
            entry: hook.entry,
            language: hook.language,
            priority: None,
            options: hook.options,
        }
    }
}

impl From<LocalHook> for HookSpec {
    fn from(hook: LocalHook) -> Self {
        Self {
            id: hook.id,
            name: hook.name,
            entry: hook.entry,
            language: hook.language,
            priority: hook.priority,
            options: hook.options,
        }
    }
}

impl From<MetaHook> for HookSpec {
    fn from(hook: MetaHook) -> Self {
        Self {
            id: hook.id,
            name: hook.name,
            entry: String::new(),
            language: Language::System,
            priority: hook.priority,
            options: hook.options,
        }
    }
}

impl From<BuiltinHook> for HookSpec {
    fn from(hook: BuiltinHook) -> Self {
        Self {
            id: hook.id,
            name: hook.name,
            entry: hook.entry,
            language: Language::System,
            priority: hook.priority,
            options: hook.options,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Repo {
    Remote {
        /// Path to the cloned repo.
        path: PathBuf,
        url: String,
        rev: String,
        hooks: Vec<HookSpec>,
    },
    Local {
        hooks: Vec<HookSpec>,
    },
    Meta {
        hooks: Vec<HookSpec>,
    },
    Builtin {
        hooks: Vec<HookSpec>,
    },
}

impl Repo {
    /// Load the remote repo manifest from the path.
    pub(crate) fn remote(url: String, rev: String, path: PathBuf) -> Result<Self, Error> {
        let manifest = read_manifest(&path.join(MANIFEST_FILE)).map_err(|e| Error::Manifest {
            repo: url.clone(),
            error: e,
        })?;
        let hooks = manifest.hooks.into_iter().map(Into::into).collect();

        Ok(Self::Remote {
            path,
            url,
            rev,
            hooks,
        })
    }

    /// Construct a local repo from a list of hooks.
    pub(crate) fn local(hooks: Vec<LocalHook>) -> Self {
        Self::Local {
            hooks: hooks.into_iter().map(Into::into).collect(),
        }
    }

    /// Construct a meta repo.
    pub(crate) fn meta(hooks: Vec<MetaHook>) -> Self {
        Self::Meta {
            hooks: hooks.into_iter().map(Into::into).collect(),
        }
    }

    /// Construct a builtin repo.
    pub(crate) fn builtin(hooks: Vec<BuiltinHook>) -> Self {
        Self::Builtin {
            hooks: hooks.into_iter().map(Into::into).collect(),
        }
    }

    /// Get the path to the cloned repo if it is a remote repo.
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Repo::Remote { path, .. } => Some(path),
            _ => None,
        }
    }

    /// Get a hook by id.
    pub(crate) fn get_hook(&self, id: &str) -> Option<&HookSpec> {
        let hooks = match self {
            Repo::Remote { hooks, .. } => hooks,
            Repo::Local { hooks } => hooks,
            Repo::Meta { hooks } => hooks,
            Repo::Builtin { hooks } => hooks,
        };
        hooks.iter().find(|hook| hook.id == id)
    }
}

impl Display for Repo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Repo::Remote { url, rev, .. } => write!(f, "{url}@{rev}"),
            Repo::Local { .. } => write!(f, "local"),
            Repo::Meta { .. } => write!(f, "meta"),
            Repo::Builtin { .. } => write!(f, "builtin"),
        }
    }
}

pub(crate) struct HookBuilder {
    project: Arc<Project>,
    repo: Arc<Repo>,
    hook_spec: HookSpec,
    // The index of the hook in the project configuration.
    idx: usize,
}

impl HookBuilder {
    pub(crate) fn new(
        project: Arc<Project>,
        repo: Arc<Repo>,
        hook_spec: HookSpec,
        idx: usize,
    ) -> Self {
        Self {
            project,
            repo,
            hook_spec,
            idx,
        }
    }

    /// Fill in the default values for the hook configuration.
    fn fill_in_defaults(&mut self) {
        let options = &mut self.hook_spec.options;
        options.language_version.get_or_insert_default();
        options.alias.get_or_insert_default();
        options.args.get_or_insert_default();
        options.env.get_or_insert_default();
        options.types.get_or_insert(vec!["file".to_string()]);
        options.types_or.get_or_insert_default();
        options.exclude_types.get_or_insert_default();
        options.always_run.get_or_insert(false);
        options.fail_fast.get_or_insert(false);
        options.pass_filenames.get_or_insert(true);
        options.require_serial.get_or_insert(false);
        options.verbose.get_or_insert(false);
        options.additional_dependencies.get_or_insert_default();
    }

    /// Check the hook configuration.
    fn check(&self) -> Result<(), Error> {
        let language = self.hook_spec.language;
        let HookOptions {
            language_version,
            additional_dependencies,
            ..
        } = &self.hook_spec.options;

        let additional_dependencies = additional_dependencies
            .as_ref()
            .map_or(&[][..], |deps| deps.as_slice());

        if !additional_dependencies.is_empty() {
            if !language.supports_install_env() {
                return Err(Error::Hook {
                    hook: self.hook_spec.id.clone(),
                    error: anyhow::anyhow!(
                        "Hook specified `additional_dependencies: {}` but the language `{}` does not install an environment",
                        additional_dependencies.join(", "),
                        language,
                    ),
                });
            }

            if !language.supports_dependency() {
                return Err(Error::Hook {
                    hook: self.hook_spec.id.clone(),
                    error: anyhow::anyhow!(
                        "Hook specified `additional_dependencies: {}` but the language `{}` does not support installing dependencies for now",
                        additional_dependencies.join(", "),
                        language,
                    ),
                });
            }
        }

        if !language.supports_language_version() {
            if let Some(language_version) = language_version
                && language_version != "default"
            {
                return Err(Error::Hook {
                    hook: self.hook_spec.id.clone(),
                    error: anyhow::anyhow!(
                        "Hook specified `language_version: {language_version}` but the language `{language}` does not support toolchain installation for now",
                    ),
                });
            }
        }

        Ok(())
    }

    /// Build the hook.
    pub(crate) async fn build(mut self) -> Result<Hook, Error> {
        // Ensure project-level defaults are applied in one place.
        // This makes call sites simpler and avoids accidental divergence.
        self.hook_spec.apply_project_defaults(self.project.config());

        self.check()?;
        self.fill_in_defaults();

        let options = self.hook_spec.options;
        let language_version = options.language_version.expect("language_version not set");
        let language_request = LanguageRequest::parse(self.hook_spec.language, &language_version)
            .map_err(|e| Error::Hook {
            hook: self.hook_spec.id.clone(),
            error: anyhow::anyhow!(e),
        })?;

        let entry = Entry::new(self.hook_spec.id.clone(), self.hook_spec.entry);

        let additional_dependencies = options
            .additional_dependencies
            .expect("additional_dependencies should not be None")
            .into_iter()
            .collect::<FxHashSet<_>>();

        let stages = match options.stages {
            Some(stages) => {
                let stages: FxHashSet<_> = stages.into_iter().collect();
                if stages.is_empty() || stages.len() == Stage::value_variants().len() {
                    Stages::All
                } else {
                    Stages::Some(stages)
                }
            }
            None => Stages::All,
        };

        let priority = self
            .hook_spec
            .priority
            .unwrap_or(u32::try_from(self.idx).expect("idx too large"));

        let mut hook = Hook {
            entry,
            stages,
            language_request,
            additional_dependencies,
            dependencies: OnceLock::new(),
            project: self.project,
            repo: self.repo,
            idx: self.idx,
            id: self.hook_spec.id,
            name: self.hook_spec.name,
            language: self.hook_spec.language,
            alias: options.alias.expect("alias not set"),
            files: options.files,
            exclude: options.exclude,
            types: options.types.expect("types not set"),
            types_or: options.types_or.expect("types_or not set"),
            exclude_types: options.exclude_types.expect("exclude_types not set"),
            args: options.args.expect("args not set"),
            env: options.env.expect("env not set"),
            always_run: options.always_run.expect("always_run not set"),
            fail_fast: options.fail_fast.expect("fail_fast not set"),
            pass_filenames: options.pass_filenames.expect("pass_filenames not set"),
            description: options.description,
            log_file: options.log_file,
            require_serial: options.require_serial.expect("require_serial not set"),
            verbose: options.verbose.expect("verbose not set"),
            minimum_prek_version: options.minimum_prek_version,
            priority,
        };

        if let Err(err) = extract_metadata_from_entry(&mut hook).await {
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() != std::io::ErrorKind::NotFound)
            {
                trace!("Failed to extract metadata from entry for hook `{hook}`: {err}");
            }
        }

        Ok(hook)
    }
}

#[derive(Debug, Clone)]
pub(crate) enum Stages {
    All,
    Some(FxHashSet<Stage>),
}

impl Stages {
    pub(crate) fn contains(&self, stage: Stage) -> bool {
        match self {
            Stages::All => true,
            Stages::Some(stages) => stages.contains(&stage),
        }
    }
}

impl Display for Stages {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Stages::All => write!(f, "all"),
            Stages::Some(stages) => {
                let stages_str = stages
                    .iter()
                    .map(Stage::as_str)
                    .collect::<Vec<_>>()
                    .join(", ");
                write!(f, "{stages_str}")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct Entry {
    hook: String,
    entry: String,
}

impl Entry {
    pub(crate) fn new(hook: String, entry: String) -> Self {
        Self { hook, entry }
    }

    /// Split the entry and resolve the command by parsing its shebang.
    pub(crate) fn resolve(&self, env_path: Option<&OsStr>) -> Result<Vec<String>, Error> {
        let split = self.split()?;

        Ok(resolve_command(split, env_path))
    }

    /// Split the entry into a list of commands.
    pub(crate) fn split(&self) -> Result<Vec<String>, Error> {
        let splits = shlex::split(&self.entry).ok_or_else(|| Error::Hook {
            hook: self.hook.clone(),
            error: anyhow::anyhow!("Failed to parse entry `{}` as commands", &self.entry),
        })?;
        if splits.is_empty() {
            return Err(Error::Hook {
                hook: self.hook.clone(),
                error: anyhow::anyhow!("Failed to parse entry: entry is empty"),
            });
        }
        Ok(splits)
    }

    /// Get the original entry string.
    pub(crate) fn raw(&self) -> &str {
        &self.entry
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub(crate) struct Hook {
    project: Arc<Project>,
    repo: Arc<Repo>,
    // Cached computed dependencies.
    dependencies: OnceLock<FxHashSet<String>>,

    /// The index of the hook defined in the configuration file.
    pub idx: usize,
    pub id: String,
    pub name: String,
    pub entry: Entry,
    pub language: Language,
    pub alias: String,
    pub files: Option<FilePattern>,
    pub exclude: Option<FilePattern>,
    pub types: Vec<String>,
    pub types_or: Vec<String>,
    pub exclude_types: Vec<String>,
    pub additional_dependencies: FxHashSet<String>,
    pub args: Vec<String>,
    pub env: FxHashMap<String, String>,
    pub always_run: bool,
    pub fail_fast: bool,
    pub pass_filenames: bool,
    pub description: Option<String>,
    pub language_request: LanguageRequest,
    pub log_file: Option<String>,
    pub require_serial: bool,
    pub stages: Stages,
    pub verbose: bool,
    pub minimum_prek_version: Option<String>,
    pub priority: u32,
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
    pub(crate) fn project(&self) -> &Project {
        &self.project
    }

    pub(crate) fn repo(&self) -> &Repo {
        &self.repo
    }

    /// Get the path to the repository that contains the hook.
    pub(crate) fn repo_path(&self) -> Option<&Path> {
        self.repo.path()
    }

    pub(crate) fn full_id(&self) -> String {
        let path = self.project.relative_path();
        if path.as_os_str().is_empty() {
            format!(".:{}", self.id)
        } else {
            format!("{}:{}", path.display(), self.id)
        }
    }

    /// Get the path where the hook should be executed.
    pub(crate) fn work_dir(&self) -> &Path {
        self.project.path()
    }

    pub(crate) fn is_remote(&self) -> bool {
        matches!(&*self.repo, Repo::Remote { .. })
    }

    /// Dependencies used to identify whether an existing hook environment can be reused.
    ///
    /// For remote hooks, the repo URL is included to avoid reusing an environment created
    /// from a different remote repository.
    pub(crate) fn env_key_dependencies(&self) -> &FxHashSet<String> {
        if !self.is_remote() {
            return &self.additional_dependencies;
        }
        self.dependencies.get_or_init(|| {
            env_key_dependencies(&self.additional_dependencies, Some(&self.repo.to_string()))
        })
    }

    /// Returns a lightweight view of the hook environment identity used for reusing installs.
    ///
    /// Returns `None` for languages that do not install an environment.
    pub(crate) fn env_key(&self) -> Option<HookEnvKeyRef<'_>> {
        if !self.language.supports_install_env() {
            return None;
        }

        Some(HookEnvKeyRef {
            language: self.language,
            dependencies: self.env_key_dependencies(),
            language_request: &self.language_request,
        })
    }

    /// Dependencies to pass to language dependency installers.
    ///
    /// For remote hooks, this includes the local path to the cloned repository so that
    /// installers can install the hook's package/project itself.
    pub(crate) fn install_dependencies(&self) -> Cow<'_, FxHashSet<String>> {
        if let Some(repo_path) = self.repo_path() {
            let mut deps = self.additional_dependencies.clone();
            deps.insert(repo_path.to_string_lossy().to_string());
            Cow::Owned(deps)
        } else {
            Cow::Borrowed(&self.additional_dependencies)
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HookEnvKey {
    pub(crate) language: Language,
    pub(crate) dependencies: FxHashSet<String>,
    pub(crate) language_request: LanguageRequest,
}

/// Borrowed form of [`HookEnvKey`] for comparing a hook to an existing installation
/// without allocating/cloning dependency sets.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HookEnvKeyRef<'a> {
    pub(crate) language: Language,
    pub(crate) dependencies: &'a FxHashSet<String>,
    pub(crate) language_request: &'a LanguageRequest,
}

/// Builds the dependency set used to identify a hook environment.
///
/// For remote hooks, `remote_repo_dependency` is included so environments from different
/// repositories are not reused accidentally.
fn env_key_dependencies(
    additional_dependencies: &FxHashSet<String>,
    remote_repo_dependency: Option<&str>,
) -> FxHashSet<String> {
    let mut deps = FxHashSet::with_capacity_and_hasher(
        additional_dependencies.len() + usize::from(remote_repo_dependency.is_some()),
        FxBuildHasher,
    );
    deps.extend(additional_dependencies.iter().cloned());
    if let Some(dep) = remote_repo_dependency {
        deps.insert(dep.to_string());
    }
    deps
}

/// Shared matching logic between a computed hook env key (owned or borrowed) and an installed
/// environment described by [`InstallInfo`].
fn matches_install_info(
    language: Language,
    dependencies: &FxHashSet<String>,
    language_request: &LanguageRequest,
    info: &InstallInfo,
) -> bool {
    info.language == language
        && info.dependencies == *dependencies
        && language_request.satisfied_by(info)
}

impl HookEnvKey {
    /// Compute the key used to match an installed hook environment.
    ///
    /// Returns `Ok(None)` if this hook does not install an environment.
    pub(crate) fn from_hook_spec(
        config: &Config,
        mut hook_spec: HookSpec,
        remote_repo_dependency: Option<&str>,
    ) -> Result<Option<Self>> {
        let language = hook_spec.language;
        if !language.supports_install_env() {
            return Ok(None);
        }

        hook_spec.apply_project_defaults(config);
        hook_spec.options.language_version.get_or_insert_default();
        hook_spec
            .options
            .additional_dependencies
            .get_or_insert_default();

        let request = hook_spec.options.language_version.as_deref().unwrap_or("");
        let language_request = LanguageRequest::parse(language, request).with_context(|| {
            format!(
                "Invalid language_version `{request}` for hook `{}`",
                hook_spec.id
            )
        })?;

        let additional_dependencies: FxHashSet<String> = hook_spec
            .options
            .additional_dependencies
            .as_ref()
            .map_or_else(FxHashSet::default, |deps| deps.iter().cloned().collect());

        let dependencies = env_key_dependencies(&additional_dependencies, remote_repo_dependency);

        Ok(Some(Self {
            language,
            dependencies,
            language_request,
        }))
    }

    pub(crate) fn matches_install_info(&self, info: &InstallInfo) -> bool {
        matches_install_info(
            self.language,
            &self.dependencies,
            &self.language_request,
            info,
        )
    }
}

impl HookEnvKeyRef<'_> {
    /// Returns true if this env key matches the given installed environment.
    pub(crate) fn matches_install_info(&self, info: &InstallInfo) -> bool {
        matches_install_info(
            self.language,
            self.dependencies,
            self.language_request,
            info,
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) enum InstalledHook {
    Installed {
        hook: Arc<Hook>,
        info: Arc<InstallInfo>,
    },
    NoNeedInstall(Arc<Hook>),
}

impl Deref for InstalledHook {
    type Target = Hook;

    fn deref(&self) -> &Self::Target {
        match self {
            InstalledHook::Installed { hook, .. } => hook,
            InstalledHook::NoNeedInstall(hook) => hook,
        }
    }
}

impl Display for InstalledHook {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        // TODO: add more information
        self.deref().fmt(f)
    }
}

pub(crate) const HOOK_MARKER: &str = ".prek-hook.json";

impl InstalledHook {
    /// Get the path to the environment where the hook is installed.
    pub(crate) fn env_path(&self) -> Option<&Path> {
        match self {
            InstalledHook::Installed { info, .. } => Some(&info.env_path),
            InstalledHook::NoNeedInstall(_) => None,
        }
    }

    /// Get the install info of the hook if it is installed.
    pub(crate) fn install_info(&self) -> Option<&InstallInfo> {
        match self {
            InstalledHook::Installed { info, .. } => Some(info),
            InstalledHook::NoNeedInstall(_) => None,
        }
    }

    /// Mark the hook as installed in the environment.
    pub(crate) async fn mark_as_installed(&self, _store: &Store) -> Result<()> {
        let Some(info) = self.install_info() else {
            return Ok(());
        };

        let content =
            serde_json::to_string_pretty(info).context("Failed to serialize install info")?;

        fs_err::tokio::write(info.env_path.join(HOOK_MARKER), content)
            .await
            .context("Failed to write install info")?;

        Ok(())
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub(crate) struct InstallInfo {
    pub(crate) language: Language,
    pub(crate) language_version: semver::Version,
    pub(crate) dependencies: FxHashSet<String>,
    pub(crate) env_path: PathBuf,
    pub(crate) toolchain: PathBuf,
    extra: FxHashMap<String, String>,
    #[serde(skip, default)]
    temp_dir: Option<TempDir>,
}

impl Clone for InstallInfo {
    fn clone(&self) -> Self {
        Self {
            language: self.language,
            language_version: self.language_version.clone(),
            dependencies: self.dependencies.clone(),
            env_path: self.env_path.clone(),
            toolchain: self.toolchain.clone(),
            extra: self.extra.clone(),
            temp_dir: None,
        }
    }
}

impl InstallInfo {
    pub(crate) fn new(
        language: Language,
        dependencies: FxHashSet<String>,
        hooks_dir: &Path,
    ) -> Result<Self, Error> {
        let env_path = tempfile::Builder::new()
            .prefix(&format!("{}-", language.as_str()))
            .rand_bytes(20)
            .tempdir_in(hooks_dir)?;

        Ok(Self {
            language,
            dependencies,
            env_path: env_path.path().to_path_buf(),
            language_version: semver::Version::new(0, 0, 0),
            toolchain: PathBuf::new(),
            extra: FxHashMap::default(),
            temp_dir: Some(env_path),
        })
    }

    pub(crate) fn persist_env_path(&mut self) {
        if let Some(temp_dir) = self.temp_dir.take() {
            self.env_path = temp_dir.keep();
        }
    }

    pub(crate) async fn from_env_path(path: &Path) -> Result<Self> {
        let content = fs_err::tokio::read_to_string(path.join(HOOK_MARKER)).await?;
        let info: InstallInfo = serde_json::from_str(&content)?;

        Ok(info)
    }

    pub(crate) async fn check_health(&self) -> Result<()> {
        self.language.check_health(self).await
    }

    pub(crate) fn with_language_version(&mut self, version: semver::Version) -> &mut Self {
        self.language_version = version;
        self
    }

    pub(crate) fn with_toolchain(&mut self, toolchain: PathBuf) -> &mut Self {
        self.toolchain = toolchain;
        self
    }

    pub(crate) fn with_extra(&mut self, key: &str, value: &str) -> &mut Self {
        self.extra.insert(key.to_string(), value.to_string());
        self
    }

    pub(crate) fn get_extra(&self, key: &str) -> Option<&String> {
        self.extra.get(key)
    }

    pub(crate) fn matches(&self, hook: &Hook) -> bool {
        hook.env_key()
            .is_some_and(|key| key.matches_install_info(self))
    }
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::sync::Arc;

    use anyhow::Result;
    use prek_consts::CONFIG_FILE;
    use rustc_hash::FxHashMap;

    use crate::config::{HookOptions, Language, RemoteHook};
    use crate::hook::HookSpec;
    use crate::workspace::Project;

    use super::{HookBuilder, Repo};

    #[tokio::test]
    async fn hook_builder_build_fills_and_merges_attributes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(CONFIG_FILE);

        // Ensure `combine()` can supply defaults for stages and language_version.
        fs_err::write(
            &config_path,
            indoc::indoc! {r"
                repos: []
                default_language_version:
                  python: python3.12
                default_stages: [manual]
            "},
        )?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local { hooks: vec![] });

        // Base hook spec (e.g. from a manifest): minimal options, one env var.
        let mut base_env = FxHashMap::default();
        base_env.insert("BASE".to_string(), "1".to_string());

        let mut hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "original-name".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            priority: None,
            options: HookOptions {
                env: Some(base_env),
                ..Default::default()
            },
        };

        // Project config overrides (e.g. from `.pre-commit-config.yaml`).
        let mut override_env = FxHashMap::default();
        override_env.insert("OVERRIDE".to_string(), "2".to_string());

        let hook_override = RemoteHook {
            id: "test-hook".to_string(),
            name: Some("override-name".to_string()),
            entry: Some("python3 -c 'print(2)'".to_string()),
            language: None,
            priority: Some(42),
            options: HookOptions {
                alias: Some("alias-1".to_string()),
                types: Some(vec!["text".to_string()]),
                args: Some(vec!["--flag".to_string()]),
                env: Some(override_env),
                always_run: Some(true),
                pass_filenames: Some(false),
                verbose: Some(true),
                description: Some("desc".to_string()),
                ..Default::default()
            },
        };

        hook_spec.apply_remote_hook_overrides(&hook_override);
        hook_spec.apply_project_defaults(project.config());

        let builder = HookBuilder::new(project.clone(), repo, hook_spec, 7);
        let hook = builder.build().await?;

        insta::assert_debug_snapshot!(hook, @r#"
        Hook {
            project: Project {
                relative_path: "",
                idx: 0,
                config: Config {
                    repos: [],
                    default_install_hook_types: None,
                    default_language_version: Some(
                        {
                            Python: "python3.12",
                        },
                    ),
                    default_stages: Some(
                        [
                            Manual,
                        ],
                    ),
                    files: None,
                    exclude: None,
                    fail_fast: None,
                    minimum_prek_version: None,
                    orphan: None,
                    _unused_keys: {},
                },
                repos: [],
                ..
            },
            repo: Local {
                hooks: [],
            },
            dependencies: OnceLock(
                <uninit>,
            ),
            idx: 7,
            id: "test-hook",
            name: "override-name",
            entry: Entry {
                hook: "test-hook",
                entry: "python3 -c 'print(2)'",
            },
            language: Python,
            alias: "alias-1",
            files: None,
            exclude: None,
            types: [
                "text",
            ],
            types_or: [],
            exclude_types: [],
            additional_dependencies: {},
            args: [
                "--flag",
            ],
            env: {
                "BASE": "1",
                "OVERRIDE": "2",
            },
            always_run: true,
            fail_fast: false,
            pass_filenames: false,
            description: Some(
                "desc",
            ),
            language_request: Python(
                MajorMinor(
                    3,
                    12,
                ),
            ),
            log_file: None,
            require_serial: false,
            stages: Some(
                {
                    Manual,
                },
            ),
            verbose: true,
            minimum_prek_version: None,
            priority: 42,
        }
        "#);

        Ok(())
    }
}
