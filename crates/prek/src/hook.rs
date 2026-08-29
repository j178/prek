use std::collections::BTreeSet;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_identify::{TagSet, tags};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use thiserror::Error;
use tracing::trace;

use crate::config::{
    self, BuiltinHook, Config, FilePattern, HookOptions, Language, LocalHook, ManifestHook,
    MetaHook, PassFilenames, RemoteHook, Stages, read_manifest,
};
use crate::hook_entry::HookEntry;
use crate::languages::version::LanguageRequest;
use crate::languages::{ShellSupport, extract_metadata};
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

/// A source-independent hook declaration.
///
/// Project defaults and language metadata are applied only when this value is consumed by
/// [`Hook::from_spec`].
#[derive(Debug)]
pub(crate) struct HookSpec {
    id: String,
    name: String,
    entry: String,
    language: Language,
    language_overridden: bool,
    priority: Option<config::Priority>,
    groups: Option<Vec<String>>,
    options: HookOptions,
}

impl HookSpec {
    pub(crate) fn from_remote(manifest: ManifestHook, config: &RemoteHook) -> Self {
        let mut spec = Self::from(manifest);

        if let Some(name) = &config.name {
            spec.name.clone_from(name);
        }
        if let Some(entry) = &config.entry {
            spec.entry.clone_from(entry);
        }
        if let Some(language) = &config.language {
            spec.language.clone_from(language);
            spec.language_overridden = true;
        }
        if config.priority.is_some() {
            spec.priority.clone_from(&config.priority);
        }
        if config.groups.is_some() {
            spec.groups.clone_from(&config.groups);
        }

        spec.options.update(&config.options);
        spec
    }

    fn with_project_defaults(mut self, config: &Config) -> Self {
        let language = self.language;
        if let Some(default) = config
            .default_language_version
            .as_ref()
            .and_then(|versions| versions.get(&language))
        {
            if let Some(language_version) = &mut self.options.language_version {
                language_version.apply_defaults(default);
            } else {
                self.options.language_version = Some(default.clone());
            }
        }

        if self
            .options
            .stages
            .as_ref()
            .is_none_or(|stages| stages.is_empty())
        {
            self.options.stages = Some(config.default_stages.unwrap_or(Stages::ALL));
        }

        if let Some(default_env) = &config.default_env {
            let mut env = default_env.clone();
            if let Some(hook_env) = self.options.env.take() {
                env.extend(hook_env);
            }
            self.options.env = Some(env);
        }

        self
    }

    fn validate(&self, repo: &Repo) -> Result<()> {
        let language = self.language;
        let additional_dependencies = self
            .options
            .additional_dependencies
            .as_deref()
            .unwrap_or_default();

        if !additional_dependencies.is_empty() {
            let dependencies = additional_dependencies.join(", ");
            if !language.supports_install_env() {
                bail!(
                    "Hook specified `additional_dependencies: {dependencies}` but the language `{language}` does not install an environment"
                );
            }

            if !language.supports_dependency() {
                bail!(
                    "Hook specified `additional_dependencies: {dependencies}` but the language `{language}` does not support installing dependencies for now"
                );
            }
        }

        if !language.supports_language_version()
            && let Some(language_version) = &self.options.language_version
            && !language_version.is_default()
        {
            bail!(
                "Hook specified `language_version: {language_version}` but the language `{language}` does not support toolchain installation for now"
            );
        }

        if self.options.shell.is_some() {
            if matches!(repo, Repo::Meta | Repo::Builtin) {
                bail!("Hook specified `shell` but {repo} hooks do not support shell execution");
            }

            if let ShellSupport::Unsupported(reason) = language.shell_support() {
                bail!(
                    "Hook specified `shell` but the language `{language}` does not support shell execution: {reason}"
                );
            }
        }

        Ok(())
    }
}

impl From<ManifestHook> for HookSpec {
    fn from(hook: ManifestHook) -> Self {
        Self {
            id: hook.id,
            name: hook.name,
            entry: hook.entry,
            language: hook.language,
            language_overridden: false,
            priority: None,
            groups: None,
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
            language_overridden: false,
            priority: hook.priority,
            groups: hook.groups,
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
            language_overridden: false,
            priority: hook.priority,
            groups: hook.groups,
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
            language_overridden: false,
            priority: hook.priority,
            groups: hook.groups,
            options: hook.options,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
pub(crate) struct RepoIdentity {
    url: String,
    rev: String,
}

impl RepoIdentity {
    pub(crate) fn as_ref(&self) -> RepoIdentityRef<'_> {
        RepoIdentityRef::new(&self.url, &self.rev)
    }
}

impl Display for RepoIdentity {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        self.as_ref().fmt(f)
    }
}

#[derive(Debug, Clone, Copy, Eq, Hash, PartialEq)]
pub(crate) struct RepoIdentityRef<'a> {
    url: &'a str,
    rev: &'a str,
}

impl<'a> RepoIdentityRef<'a> {
    pub(crate) fn new(url: &'a str, rev: &'a str) -> Self {
        Self { url, rev }
    }
}

impl Display for RepoIdentityRef<'_> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.url, self.rev)
    }
}

impl From<RepoIdentityRef<'_>> for RepoIdentity {
    fn from(value: RepoIdentityRef<'_>) -> Self {
        Self {
            url: value.url.to_string(),
            rev: value.rev.to_string(),
        }
    }
}

#[derive(Debug)]
pub(crate) enum Repo {
    Remote {
        /// Path to the cloned repo.
        path: PathBuf,
        url: String,
        rev: String,
        hooks: Vec<ManifestHook>,
    },
    Local,
    Meta,
    Builtin,
}

impl Repo {
    /// Load the remote repo manifest from the path.
    pub(crate) fn remote(url: String, rev: String, path: PathBuf) -> Result<Self, Error> {
        let manifest =
            read_manifest(&path.join(PRE_COMMIT_HOOKS_YAML)).map_err(|e| Error::Manifest {
                repo: url.clone(),
                error: e,
            })?;
        Ok(Self::Remote {
            path,
            url,
            rev,
            hooks: manifest.hooks,
        })
    }

    /// Get the path to the cloned repo if it is a remote repo.
    pub(crate) fn path(&self) -> Option<&Path> {
        match self {
            Repo::Remote { path, .. } => Some(path),
            _ => None,
        }
    }

    pub(crate) fn identity(&self) -> Option<RepoIdentityRef<'_>> {
        match self {
            Repo::Remote { url, rev, .. } => Some(RepoIdentityRef::new(url, rev)),
            Repo::Local | Repo::Meta | Repo::Builtin => None,
        }
    }

    /// Get a hook declaration from a remote repository manifest.
    pub(crate) fn manifest_hook(&self, id: &str) -> Option<&ManifestHook> {
        let Self::Remote { hooks, .. } = self else {
            return None;
        };
        hooks.iter().find(|hook| hook.id == id)
    }
}

impl Display for Repo {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Repo::Remote { url, rev, .. } => write!(f, "{url}@{rev}"),
            Repo::Local => write!(f, "local"),
            Repo::Meta => write!(f, "meta"),
            Repo::Builtin => write!(f, "builtin"),
        }
    }
}

fn hook_error(hook: &str, error: impl Into<anyhow::Error>) -> Error {
    Error::Hook {
        hook: hook.to_string(),
        error: error.into(),
    }
}

/// Workspace-local identity of a configured hook.
///
/// Hook indexes are scoped to a project config, so the project index is part of the key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct HookKey {
    pub(crate) project_idx: usize,
    pub(crate) hook_idx: usize,
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub(crate) struct Hook {
    project: Arc<Project>,
    repo: Arc<Repo>,

    /// The index of the hook defined in the configuration file.
    pub idx: usize,
    pub id: String,
    pub name: String,
    pub entry: HookEntry,
    pub language: Language,
    pub(crate) language_overridden: bool,
    pub alias: String,
    pub files: Option<FilePattern>,
    pub exclude: Option<FilePattern>,
    pub types: TagSet,
    pub types_or: TagSet,
    pub exclude_types: TagSet,
    pub additional_dependencies: Vec<String>,
    pub args: Vec<String>,
    pub env: FxHashMap<String, String>,
    pub always_run: bool,
    pub fail_fast: bool,
    pub pass_filenames: PassFilenames,
    pub description: Option<String>,
    pub language_request: LanguageRequest,
    pub log_file: Option<String>,
    pub require_serial: bool,
    pub stages: Stages,
    pub verbose: bool,
    pub minimum_prek_version: Option<String>,
    pub priority: u32,
    pub groups: BTreeSet<String>,
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
    /// Resolve a merged hook specification into an executable hook.
    pub(crate) async fn from_spec(
        project: Arc<Project>,
        repo: Arc<Repo>,
        hook_spec: HookSpec,
        idx: usize,
    ) -> Result<Self, Error> {
        let hook_spec = hook_spec.with_project_defaults(project.config());
        hook_spec
            .validate(&repo)
            .map_err(|error| hook_error(&hook_spec.id, error))?;

        let HookSpec {
            id,
            name,
            entry: raw_entry,
            language,
            language_overridden,
            priority,
            groups,
            options,
        } = hook_spec;

        let priority = match priority {
            Some(priority) => priority.resolve(&project.config().priorities, &id)?,
            None => u32::try_from(idx).expect("hook index should fit in u32"),
        };
        let groups = groups
            .unwrap_or_default()
            .into_iter()
            .collect::<BTreeSet<_>>();

        let HookOptions {
            alias,
            files,
            exclude,
            types,
            types_or,
            exclude_types,
            additional_dependencies,
            args,
            env,
            always_run,
            fail_fast,
            pass_filenames,
            description,
            language_version,
            log_file,
            shell,
            require_serial,
            stages,
            verbose,
            minimum_prek_version,
            _unused_keys: _,
        } = options;

        let language_request = LanguageRequest::from_config(language, language_version.as_ref())
            .map_err(|error| hook_error(&id, error))?;
        let entry = HookEntry::new(id.clone(), raw_entry, shell);

        let mut hook = Self {
            project,
            repo,
            idx,
            id,
            name,
            entry,
            language,
            language_overridden,
            alias: alias.unwrap_or_default(),
            files,
            exclude,
            types: types.unwrap_or(tags::TAG_SET_FILE),
            types_or: types_or.unwrap_or_default(),
            exclude_types: exclude_types.unwrap_or_default(),
            additional_dependencies: additional_dependencies.unwrap_or_default(),
            args: args.unwrap_or_default(),
            env: env.unwrap_or_default(),
            always_run: always_run.unwrap_or(false),
            fail_fast: fail_fast.unwrap_or(false),
            pass_filenames: pass_filenames.unwrap_or(PassFilenames::All),
            description,
            language_request,
            log_file,
            require_serial: require_serial.unwrap_or(false),
            stages: stages.unwrap_or(Stages::ALL),
            verbose: verbose.unwrap_or(false),
            minimum_prek_version,
            priority,
            groups,
        };

        if let Err(err) = extract_metadata(&mut hook).await
            && err
                .downcast_ref::<std::io::Error>()
                .is_none_or(|error| error.kind() != std::io::ErrorKind::NotFound)
        {
            trace!("Failed to extract metadata from entry for hook `{hook}`: {err}");
        }

        Ok(hook)
    }

    pub(crate) fn project(&self) -> &Project {
        &self.project
    }

    pub(crate) fn key(&self) -> HookKey {
        HookKey {
            project_idx: self.project.idx(),
            hook_idx: self.idx,
        }
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

    pub(crate) fn needs_install_env(&self) -> bool {
        !matches!(self.repo(), Repo::Meta | Repo::Builtin) && self.language.supports_install_env()
    }

    /// Returns a lightweight view of the hook's environment requirement.
    ///
    /// Returns `None` for hooks that do not install an environment.
    pub(crate) fn environment_requirement(&self) -> Option<HookEnvRequirementRef<'_>> {
        if !self.needs_install_env() {
            return None;
        }

        Some(HookEnvRequirementRef {
            language: self.language,
            repo: self.repo.identity(),
            dependencies: &self.additional_dependencies,
            language_request: &self.language_request,
        })
    }
}

#[derive(Debug)]
pub(crate) struct HookEnvRequirement {
    pub(crate) language: Language,
    repo: Option<RepoIdentity>,
    dependencies: Vec<String>,
    language_request: LanguageRequest,
}

/// Borrowed form of [`HookEnvRequirement`] for comparing a hook to an existing installation
/// without allocating or cloning the dependency sequence.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HookEnvRequirementRef<'a> {
    language: Language,
    repo: Option<RepoIdentityRef<'a>>,
    dependencies: &'a [String],
    language_request: &'a LanguageRequest,
}

impl HookEnvRequirement {
    /// Compute the requirement used to match an installed hook environment.
    ///
    /// Returns `Ok(None)` if this hook does not install an environment.
    pub(crate) fn from_hook_spec(
        config: &Config,
        hook_spec: HookSpec,
        repo: Option<RepoIdentityRef<'_>>,
    ) -> Result<Option<Self>> {
        let hook_spec = hook_spec.with_project_defaults(config);
        let language = hook_spec.language;
        if !language.supports_install_env() {
            return Ok(None);
        }

        let language_request =
            LanguageRequest::from_config(language, hook_spec.options.language_version.as_ref())
                .with_context(|| format!("Invalid language_version for hook `{}`", hook_spec.id))?;
        let dependencies = hook_spec
            .options
            .additional_dependencies
            .unwrap_or_default();

        Ok(Some(Self {
            language,
            repo: repo.map(RepoIdentity::from),
            dependencies,
            language_request,
        }))
    }

    pub(crate) fn as_ref(&self) -> HookEnvRequirementRef<'_> {
        HookEnvRequirementRef {
            language: self.language,
            repo: self.repo.as_ref().map(RepoIdentity::as_ref),
            dependencies: &self.dependencies,
            language_request: &self.language_request,
        }
    }
}

impl HookEnvRequirementRef<'_> {
    /// Returns true if the installed environment satisfies this requirement.
    pub(crate) fn is_satisfied_by(&self, info: &InstallInfo) -> bool {
        info.is_current_schema()
            && info.language == self.language
            && info.repo.as_ref().map(RepoIdentity::as_ref) == self.repo
            && info.dependencies == self.dependencies
            && self.language_request.satisfied_by(info)
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
pub(crate) const INSTALL_INFO_SCHEMA_VERSION: u8 = 1;

impl InstalledHook {
    /// Get the path to the environment where the hook is installed.
    pub(crate) fn env_path(&self) -> Option<&Path> {
        match self {
            InstalledHook::Installed { info, .. } => Some(&info.env_path),
            InstalledHook::NoNeedInstall(_) => None,
        }
    }

    /// Get the directory the toolchain is installed in.
    pub(crate) fn toolchain_dir(&self) -> Option<&Path> {
        match self {
            InstalledHook::Installed { info, .. } => info.toolchain.parent(),
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
    schema_version: u8,
    pub(crate) language: Language,
    pub(crate) language_version: semver::Version,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repo: Option<RepoIdentity>,
    pub(crate) dependencies: Vec<String>,
    pub(crate) env_path: PathBuf,
    pub(crate) toolchain: PathBuf,
    extra: FxHashMap<String, String>,
    #[serde(skip, default)]
    temp_dir: Option<TempDir>,
}

impl Clone for InstallInfo {
    fn clone(&self) -> Self {
        Self {
            schema_version: self.schema_version,
            language: self.language,
            language_version: self.language_version.clone(),
            repo: self.repo.clone(),
            dependencies: self.dependencies.clone(),
            env_path: self.env_path.clone(),
            toolchain: self.toolchain.clone(),
            extra: self.extra.clone(),
            temp_dir: None,
        }
    }
}

impl InstallInfo {
    pub(crate) fn new(hook: &Hook, hooks_dir: &Path) -> Result<Self, Error> {
        Self::create(
            hook.language,
            hook.repo.identity().map(RepoIdentity::from),
            hook.additional_dependencies.clone(),
            hooks_dir,
        )
    }

    pub(crate) fn create(
        language: Language,
        repo: Option<RepoIdentity>,
        dependencies: Vec<String>,
        hooks_dir: &Path,
    ) -> Result<Self, Error> {
        let env_path = tempfile::Builder::new()
            .prefix(&format!("{language}-"))
            .rand_bytes(20)
            .tempdir_in(hooks_dir)?;

        Ok(Self {
            schema_version: INSTALL_INFO_SCHEMA_VERSION,
            language,
            repo,
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

    pub(crate) fn is_current_schema(&self) -> bool {
        self.schema_version == INSTALL_INFO_SCHEMA_VERSION
    }

    pub(crate) fn repo(&self) -> Option<RepoIdentityRef<'_>> {
        self.repo.as_ref().map(RepoIdentity::as_ref)
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
}

#[cfg(test)]
mod tests {
    use std::borrow::Cow;
    use std::path::PathBuf;
    use std::sync::Arc;

    use anyhow::Result;
    use prek_consts::PRE_COMMIT_CONFIG_YAML;
    use prek_identify::tags;
    use rustc_hash::FxHashMap;
    use serde_json::json;

    use crate::config::{
        Config, HookOptions, Language, LanguageVersion, ManifestHook, PassFilenames, Priority,
        RemoteHook, Shell, Stage, Stages,
    };
    use crate::hook::HookSpec;
    use crate::hooks::check_fast_path;
    use crate::languages::version::LanguageRequest;
    use crate::workspace::Project;

    use super::{
        Hook, HookEnvRequirementRef, INSTALL_INFO_SCHEMA_VERSION, InstallInfo, Repo,
        RepoIdentityRef,
    };

    #[tokio::test]
    async fn hook_from_spec_fills_and_merges_attributes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);

        // Ensure project defaults are applied to hooks.
        fs_err::write(
            &config_path,
            indoc::indoc! {r"
                repos: []
                default_language_version:
                  python: python3.12
                default_stages: [manual]
                default_env:
                  BASE: default
                  DEFAULT_ONLY: 1
                  OVERRIDE: default
            "},
        )?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local);

        // Base hook spec (e.g. from a manifest): options that config can merge or override.
        let mut base_env = FxHashMap::default();
        base_env.insert("BASE".to_string(), "1".to_string());

        let manifest_hook = ManifestHook {
            id: "test-hook".to_string(),
            name: "original-name".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            options: HookOptions {
                env: Some(base_env),
                shell: Some(Shell::Sh),
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
            priority: Some(Priority::Number(42)),
            groups: Some(vec!["ci".to_string(), "format".to_string()]),
            options: HookOptions {
                alias: Some("alias-1".to_string()),
                types: Some(tags::TAG_SET_TEXT),
                args: Some(vec!["--flag".to_string()]),
                env: Some(override_env),
                always_run: Some(true),
                pass_filenames: Some(PassFilenames::None),
                verbose: Some(true),
                description: Some("desc".to_string()),
                shell: Some(Shell::Bash),
                ..Default::default()
            },
        };

        let hook_spec = HookSpec::from_remote(manifest_hook, &hook_override);
        let hook = Hook::from_spec(project.clone(), repo, hook_spec, 7).await?;

        insta::assert_debug_snapshot!(hook, @r#"
        Hook {
            project: Project {
                relative_path: "",
                idx: 0,
                config: Config {
                    update: None,
                    priorities: {},
                    repos: [],
                    default_install_hook_types: None,
                    default_language_version: Some(
                        {
                            Python: LanguageVersion {
                                request: Explicit(
                                    "python3.12",
                                ),
                                preference: None,
                                allows_download: true,
                            },
                        },
                    ),
                    default_stages: Some(
                        Stages(manual),
                    ),
                    default_env: Some(
                        {
                            "BASE": "default",
                            "OVERRIDE": "default",
                            "DEFAULT_ONLY": "1",
                        },
                    ),
                    files: None,
                    exclude: None,
                    fail_fast: None,
                    minimum_prek_version: None,
                    orphan: None,
                    _unused_keys: {},
                },
                ..
            },
            repo: Local,
            idx: 7,
            id: "test-hook",
            name: "override-name",
            entry: Shell(
                ShellHookEntry {
                    hook: "test-hook",
                    entry: "python3 -c 'print(2)'",
                    shell: Bash,
                },
            ),
            language: Python,
            language_overridden: false,
            alias: "alias-1",
            files: None,
            exclude: None,
            types: [
                "text",
            ],
            types_or: [],
            exclude_types: [],
            additional_dependencies: [],
            args: [
                "--flag",
            ],
            env: {
                "BASE": "1",
                "DEFAULT_ONLY": "1",
                "OVERRIDE": "2",
            },
            always_run: true,
            fail_fast: false,
            pass_filenames: None,
            description: Some(
                "desc",
            ),
            language_request: LanguageRequest {
                version: Python(
                    MajorMinor(
                        3,
                        12,
                    ),
                ),
                preference: Managed,
                allows_download: true,
            },
            log_file: None,
            require_serial: false,
            stages: Stages(manual),
            verbose: true,
            minimum_prek_version: None,
            priority: 42,
            groups: {
                "ci",
                "format",
            },
        }
        "#);

        Ok(())
    }

    #[tokio::test]
    async fn explicit_language_override_uses_pinned_remote_hook() -> Result<()> {
        let (temp, project) = setup_hook_test()?;
        let repo = Arc::new(Repo::Remote {
            path: temp.path().join("remote-repo"),
            url: "https://github.com/pre-commit/pre-commit-hooks".to_string(),
            rev: "v6.0.0".to_string(),
            hooks: vec![],
        });
        let manifest_hook = ManifestHook {
            id: "check-yaml".to_string(),
            name: "check yaml".to_string(),
            entry: "check-yaml".to_string(),
            language: Language::Python,
            options: HookOptions::default(),
        };
        let hook_override = RemoteHook {
            id: "check-yaml".to_string(),
            name: None,
            entry: None,
            language: Some(Language::Python),
            priority: None,
            groups: None,
            options: HookOptions::default(),
        };

        let hook_spec = HookSpec::from_remote(manifest_hook, &hook_override);
        let hook = Hook::from_spec(project, repo, hook_spec, 0).await?;

        assert!(!check_fast_path(&hook));
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_empty_hook_stages_inherit_default_stages() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\ndefault_stages: [manual]\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local);

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            language_overridden: false,
            priority: None,
            groups: None,
            options: HookOptions {
                stages: Some(Stages::from([])),
                ..Default::default()
            },
        };

        let hook = Hook::from_spec(project, repo, hook_spec, 0).await?;

        assert_eq!(hook.stages, Stages::from([Stage::Manual]));
        Ok(())
    }

    #[test]
    fn hook_spec_with_project_defaults_sets_explicit_all_when_default_stages_missing() {
        let config: Config = serde_saphyr::from_str("repos: []\n").expect("config should parse");

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            language_overridden: false,
            priority: None,
            groups: None,
            options: HookOptions::default(),
        };

        let hook_spec = hook_spec.with_project_defaults(&config);

        assert_eq!(hook_spec.options.stages, Some(Stages::ALL));
    }

    #[test]
    fn hook_spec_with_project_defaults_merges_language_version_fields() {
        let config: Config = serde_saphyr::from_str(indoc::indoc! {r"
            repos: []
            default_language_version:
              python:
                request: '3.12'
                preference: only-managed
        "})
        .expect("config should parse");
        let language_version: LanguageVersion =
            serde_saphyr::from_str("preference: system\n").expect("version should parse");
        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python -m test".to_string(),
            language: Language::Python,
            language_overridden: false,
            priority: None,
            groups: None,
            options: HookOptions {
                language_version: Some(language_version),
                ..Default::default()
            },
        }
        .with_project_defaults(&config);

        let language_version = hook_spec.options.language_version.as_ref().unwrap();
        assert_eq!(language_version.request(), Some("3.12"));
        assert_eq!(
            language_version.preference(),
            crate::config::ToolchainPreference::System
        );
    }

    #[tokio::test]
    async fn hook_from_spec_preserves_explicit_empty_default_stages() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\ndefault_stages: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local);

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            language_overridden: false,
            priority: None,
            groups: None,
            options: HookOptions::default(),
        };

        let hook = Hook::from_spec(project, repo, hook_spec, 0).await?;

        assert_eq!(hook.stages, Stages::from([]));
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_defaults_to_all_when_stages_and_default_stages_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local);

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            language_overridden: false,
            priority: None,
            groups: None,
            options: HookOptions::default(),
        };

        let hook = Hook::from_spec(project, repo, hook_spec, 0).await?;

        assert_eq!(hook.stages, Stages::ALL);
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_empty_hook_stages_default_to_all_when_default_stages_missing()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local);

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            language_overridden: false,
            priority: None,
            groups: None,
            options: HookOptions {
                stages: Some(Stages::from([])),
                ..Default::default()
            },
        };

        let hook = Hook::from_spec(project, repo, hook_spec, 0).await?;

        assert_eq!(hook.stages, Stages::ALL);
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_preserves_additional_dependency_order() -> Result<()> {
        let (temp, project) = setup_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        let repo = Arc::new(Repo::Remote {
            path: repo_path.clone(),
            url: "https://example.invalid/hooks".to_string(),
            rev: "v0.1.0".to_string(),
            hooks: vec![],
        });
        let repo_identity = RepoIdentityRef::new("https://example.invalid/hooks", "v0.1.0");
        let additional_dependencies = vec![
            "dep-c".to_string(),
            "dep-a".to_string(),
            "dep-c".to_string(),
        ];

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "./hook.py".to_string(),
            language: Language::Python,
            language_overridden: false,
            priority: None,
            groups: None,
            options: HookOptions {
                additional_dependencies: Some(additional_dependencies.clone()),
                ..Default::default()
            },
        };

        let hook = Hook::from_spec(project, repo, hook_spec, 0).await?;
        let requirement = hook
            .environment_requirement()
            .expect("Python hook installs an environment");

        assert_eq!(hook.additional_dependencies, additional_dependencies);
        assert_eq!(requirement.repo, Some(repo_identity));
        assert_eq!(requirement.dependencies, additional_dependencies);

        let hooks_dir = temp.path().join("hooks");
        fs_err::create_dir(&hooks_dir)?;
        let install_info = InstallInfo::new(&hook, &hooks_dir)?;
        assert!(install_info.is_current_schema());
        assert_eq!(install_info.repo(), Some(repo_identity));
        assert_eq!(install_info.dependencies, additional_dependencies);
        assert!(requirement.is_satisfied_by(&install_info));

        let marker = serde_json::to_value(&install_info)?;
        assert_eq!(marker["schema_version"], json!(INSTALL_INFO_SCHEMA_VERSION));
        assert_eq!(
            marker["repo"],
            json!({
                "url": "https://example.invalid/hooks",
                "rev": "v0.1.0",
            })
        );
        Ok(())
    }

    #[test]
    fn install_info_requires_schema_version() {
        let err = serde_json::from_value::<InstallInfo>(json!({
            "language": "python",
            "language_version": "3.12.0",
            "dependencies": ["dep"],
            "env_path": "/tmp/legacy-env",
            "toolchain": "/usr/bin/python3",
            "extra": {},
        }))
        .unwrap_err();

        assert_eq!(err.to_string(), "missing field `schema_version`");
    }

    #[test]
    fn structured_repo_distinguishes_remote_and_local_requirements() -> Result<()> {
        let repo = RepoIdentityRef::new("https://example.invalid/hooks", "v0.1.0");
        let repo_like_dependency = repo.to_string();
        let install_info: InstallInfo = serde_json::from_value(json!({
            "schema_version": INSTALL_INFO_SCHEMA_VERSION,
            "language": "python",
            "language_version": "3.12.0",
            "repo": {
                "url": "https://example.invalid/hooks",
                "rev": "v0.1.0",
            },
            "dependencies": [repo_like_dependency],
            "env_path": "/tmp/remote-env",
            "toolchain": "/usr/bin/python3",
            "extra": {},
        }))?;
        let dependencies = vec![repo.to_string()];
        let language_request = LanguageRequest::parse(Language::Python, "")?;
        let remote_requirement = HookEnvRequirementRef {
            language: Language::Python,
            repo: Some(repo),
            dependencies: &dependencies,
            language_request: &language_request,
        };
        let local_requirement = HookEnvRequirementRef {
            repo: None,
            ..remote_requirement
        };

        assert!(remote_requirement.is_satisfied_by(&install_info));
        assert!(!local_requirement.is_satisfied_by(&install_info));
        Ok(())
    }

    #[test]
    fn toolchain_preferences_do_not_affect_existing_environment_reuse() -> Result<()> {
        let dependencies = Vec::new();
        let install_info: InstallInfo = serde_json::from_value(json!({
            "schema_version": INSTALL_INFO_SCHEMA_VERSION,
            "language": "python",
            "language_version": "3.12.0",
            "dependencies": [],
            "env_path": "/tmp/python-env",
            "toolchain": "/usr/bin/python3",
            "extra": {},
        }))?;

        for preference in ["only-managed", "managed", "system", "only-system"] {
            let language_version: LanguageVersion =
                serde_saphyr::from_str(&format!("preference: {preference}\n"))?;
            let language_request =
                LanguageRequest::from_config(Language::Python, Some(&language_version))?;
            let requirement = HookEnvRequirementRef {
                language: Language::Python,
                repo: None,
                dependencies: &dependencies,
                language_request: &language_request,
            };

            assert!(requirement.is_satisfied_by(&install_info), "{preference}");
        }
        Ok(())
    }

    /// Set up a temporary directory with a minimal `.pre-commit-config.yaml`
    /// and a `remote-repo` subdirectory.
    fn setup_hook_test() -> Result<(tempfile::TempDir, Arc<Project>)> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);

        let repo_path = temp.path().join("remote-repo");
        fs_err::create_dir_all(&repo_path)?;

        Ok((temp, project))
    }

    /// Build a hook from the given repo path and options.
    async fn build_hook(
        project: Arc<Project>,
        repo_path: PathBuf,
        language: Language,
        entry: &str,
        language_version: Option<&str>,
    ) -> Result<Hook> {
        let repo = Arc::new(Repo::Remote {
            path: repo_path,
            url: "https://example.invalid/hooks".to_string(),
            rev: "v0.1.0".to_string(),
            hooks: vec![],
        });

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: entry.to_string(),
            language,
            language_overridden: false,
            priority: None,
            groups: None,
            options: HookOptions {
                language_version: language_version.map(LanguageVersion::from),
                ..Default::default()
            },
        };

        Ok(Hook::from_spec(project, repo, hook_spec, 0).await?)
    }

    async fn build_python_hook(
        project: Arc<Project>,
        repo_path: PathBuf,
        language_version: Option<&str>,
    ) -> Result<Hook> {
        build_hook(
            project,
            repo_path,
            Language::Python,
            "./hook.py",
            language_version,
        )
        .await
    }

    static PEP723_SCRIPT: &str = indoc::indoc! {r#"
        # /// script
        # requires-python = ">=3.11"
        # ///
        print("hello")
    "#};

    #[tokio::test]
    async fn hook_from_spec_python_pep723_overrides_user_and_pyproject() -> Result<()> {
        let (temp, project) = setup_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(
            repo_path.join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.8\"\n",
        )?;
        fs_err::write(repo_path.join("hook.py"), PEP723_SCRIPT)?;

        let hook = build_python_hook(project, repo_path, Some("3.9")).await?;

        assert_eq!(
            hook.language_request,
            LanguageRequest::parse(Language::Python, ">=3.11")?
        );
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_python_user_language_version_overrides_pyproject() -> Result<()> {
        let (temp, project) = setup_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(
            repo_path.join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.11\"\n",
        )?;
        fs_err::write(repo_path.join("hook.py"), "print(\"hello\")\n")?;

        let hook = build_python_hook(project, repo_path, Some("3.9")).await?;

        assert_eq!(
            hook.language_request,
            LanguageRequest::parse(Language::Python, "3.9")?
        );
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_python_pep723_overrides_pyproject_without_user_version() -> Result<()> {
        let (temp, project) = setup_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(
            repo_path.join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.8\"\n",
        )?;
        fs_err::write(repo_path.join("hook.py"), PEP723_SCRIPT)?;

        let hook = build_python_hook(project, repo_path, None).await?;

        assert_eq!(
            hook.language_request,
            LanguageRequest::parse(Language::Python, ">=3.11")?
        );
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_python_metadata_refines_system_without_enabling_downloads() -> Result<()>
    {
        let (temp, project) = setup_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(
            repo_path.join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.8\"\n",
        )?;
        fs_err::write(repo_path.join("hook.py"), PEP723_SCRIPT)?;

        let hook = build_python_hook(project, repo_path, Some("system")).await?;
        let expected = LanguageRequest::parse(Language::Python, ">=3.11")?;

        assert_eq!(
            hook.language_request.version_request(),
            expected.version_request()
        );
        assert!(!hook.language_request.toolchain_policy().allows_download());
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_python_defaults_to_any_without_version_sources() -> Result<()> {
        let (temp, project) = setup_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(repo_path.join("hook.py"), "print(\"hello\")\n")?;

        let hook = build_python_hook(project, repo_path, None).await?;

        assert!(hook.language_request.is_any());
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_python_pyproject_provides_version_when_no_other_source() -> Result<()> {
        let (temp, project) = setup_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(
            repo_path.join("pyproject.toml"),
            "[project]\nrequires-python = \">=3.10\"\n",
        )?;
        fs_err::write(repo_path.join("hook.py"), "print(\"hello\")\n")?;

        let hook = build_python_hook(project, repo_path, None).await?;

        assert_eq!(
            hook.language_request,
            LanguageRequest::parse(Language::Python, ">=3.10")?
        );
        Ok(())
    }

    #[tokio::test]
    async fn hook_from_spec_go_mod_refines_system_without_enabling_downloads() -> Result<()> {
        let (temp, project) = setup_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(
            repo_path.join("go.mod"),
            "module example.com/test-hook\n\ngo 1.22\n",
        )?;

        let hook = build_hook(
            project,
            repo_path,
            Language::Golang,
            "go test",
            Some("system"),
        )
        .await?;
        let expected = LanguageRequest::parse(Language::Golang, ">= 1.22.0")?;

        assert_eq!(
            hook.language_request.version_request(),
            expected.version_request()
        );
        assert!(!hook.language_request.toolchain_policy().allows_download());
        Ok(())
    }
}
