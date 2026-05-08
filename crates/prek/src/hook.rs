use std::borrow::Cow;
use std::fmt::{Display, Formatter};
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_identify::{TagSet, tags};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use thiserror::Error;
use tracing::trace;

use crate::config::{
    self, BuiltinHook, Config, FilePattern, HookOptions, Language, LocalHook, ManifestHook,
    MetaHook, PassFilenames, RemoteHook, Stages, read_manifest,
};
use crate::hook_entry::HookEntry;
use crate::hook_env::{HookEnvKey, HookEnvSpec, PythonUvEnv};
use crate::languages::version::LanguageRequest;
use crate::languages::{HookMetadata, ShellSupport, extract_metadata};
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

        if self
            .options
            .stages
            .as_ref()
            .is_none_or(|stages| stages.is_empty())
        {
            self.options.stages = Some(config.default_stages.unwrap_or(Stages::ALL));
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
        let manifest =
            read_manifest(&path.join(PRE_COMMIT_HOOKS_YAML)).map_err(|e| Error::Manifest {
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

    /// Check the hook configuration.
    fn check(&self) -> Result<(), Error> {
        let language = self.hook_spec.language;
        let HookOptions {
            language_version,
            shell,
            ..
        } = &self.hook_spec.options;

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

        if shell.is_some() {
            match self.repo.as_ref() {
                Repo::Meta { .. } => {
                    return Err(Error::Hook {
                        hook: self.hook_spec.id.clone(),
                        error: anyhow::anyhow!(
                            "Hook specified `shell` but meta hooks do not support shell execution",
                        ),
                    });
                }
                Repo::Builtin { .. } => {
                    return Err(Error::Hook {
                        hook: self.hook_spec.id.clone(),
                        error: anyhow::anyhow!(
                            "Hook specified `shell` but builtin hooks do not support shell execution",
                        ),
                    });
                }
                Repo::Remote { .. } | Repo::Local { .. } => {}
            }

            if let ShellSupport::Unsupported(reason) = language.shell_support() {
                return Err(Error::Hook {
                    hook: self.hook_spec.id.clone(),
                    error: anyhow::anyhow!(
                        "Hook specified `shell` but the language `{language}` does not support shell execution: {reason}",
                    ),
                });
            }
        }

        Ok(())
    }

    /// Build the hook.
    pub(crate) async fn build(mut self) -> Result<Hook, Error> {
        self.hook_spec.apply_project_defaults(self.project.config());

        let remote_repo_dependency = self
            .hook_spec
            .language
            .supports_install_env()
            .then(|| match self.repo.as_ref() {
                Repo::Remote { .. } => Some(self.repo.to_string()),
                Repo::Local { .. } | Repo::Meta { .. } | Repo::Builtin { .. } => None,
            })
            .flatten();

        self.check()?;

        let options = self.hook_spec.options;
        let language_version = options.language_version.unwrap_or_default();
        let uv = options.uv;
        let alias = options.alias.unwrap_or_default();
        let args = options.args.unwrap_or_default();
        let env = options.env.unwrap_or_default();
        let types = options.types.unwrap_or(tags::TAG_SET_FILE);
        let types_or = options.types_or.unwrap_or_default();
        let exclude_types = options.exclude_types.unwrap_or_default();
        let always_run = options.always_run.unwrap_or(false);
        let fail_fast = options.fail_fast.unwrap_or(false);
        let pass_filenames = options.pass_filenames.unwrap_or(PassFilenames::All);
        let require_serial = options.require_serial.unwrap_or(false);
        let verbose = options.verbose.unwrap_or(false);
        let stages = options.stages.unwrap_or(Stages::ALL);
        let shell = options.shell;
        let mut additional_dependencies = options
            .additional_dependencies
            .unwrap_or_default()
            .into_iter()
            .collect::<FxHashSet<_>>();

        let mut language_request =
            LanguageRequest::parse(self.hook_spec.language, &language_version).map_err(|e| {
                Error::Hook {
                    hook: self.hook_spec.id.clone(),
                    error: anyhow::anyhow!(e),
                }
            })?;

        let entry = HookEntry::new(self.hook_spec.id.clone(), self.hook_spec.entry, shell);

        let priority = self
            .hook_spec
            .priority
            .unwrap_or(u32::try_from(self.idx).expect("idx too large"));

        let mut metadata = HookMetadata {
            id: &self.hook_spec.id,
            language: self.hook_spec.language,
            entry: &entry,
            repo_path: self.repo.path(),
            work_dir: self.project.path(),
            additional_dependencies: &mut additional_dependencies,
            language_request: &mut language_request,
        };

        if let Err(err) = extract_metadata(&mut metadata).await {
            if err
                .downcast_ref::<std::io::Error>()
                .is_some_and(|e| e.kind() != std::io::ErrorKind::NotFound)
            {
                trace!(
                    "Failed to extract metadata from entry for hook `{}`: {err}",
                    self.hook_spec.id
                );
            }
        }

        let env_spec = HookEnvSpec::resolve(
            self.hook_spec.language,
            &additional_dependencies,
            uv.as_ref(),
            self.project.path(),
            remote_repo_dependency.as_deref(),
        )
        .map_err(|e| Error::Hook {
            hook: self.hook_spec.id.clone(),
            error: e,
        })?;

        Ok(Hook {
            project: self.project,
            repo: self.repo,
            env_spec,
            idx: self.idx,
            id: self.hook_spec.id,
            name: self.hook_spec.name,
            language: self.hook_spec.language,

            priority,
            entry,
            stages,
            language_request,
            additional_dependencies,
            alias,
            types,
            types_or,
            exclude_types,
            args,
            env,
            always_run,
            fail_fast,
            pass_filenames,
            require_serial,
            verbose,
            files: options.files,
            exclude: options.exclude,
            description: options.description,
            log_file: options.log_file,
            minimum_prek_version: options.minimum_prek_version,
        })
    }
}

#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Clone)]
pub(crate) struct Hook {
    project: Arc<Project>,
    repo: Arc<Repo>,
    env_spec: HookEnvSpec,

    /// The index of the hook defined in the configuration file.
    pub idx: usize,
    pub id: String,
    pub name: String,
    pub entry: HookEntry,
    pub language: Language,
    pub alias: String,
    pub files: Option<FilePattern>,
    pub exclude: Option<FilePattern>,
    pub types: TagSet,
    pub types_or: TagSet,
    pub exclude_types: TagSet,
    pub additional_dependencies: FxHashSet<String>,
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
        self.env_spec.dependencies()
    }

    pub(crate) fn python_uv_env(&self) -> Option<&PythonUvEnv> {
        self.env_spec.python_uv()
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
            .prefix(&format!("{language}-"))
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

    /// Returns whether this installed environment can be reused for the hook.
    ///
    /// Used when a fully resolved hook is available, such as install reuse
    /// checks before running a hook.
    pub(crate) fn matches(&self, hook: &Hook) -> bool {
        hook.language.supports_install_env()
            && self.language == hook.language
            && self.dependencies == *hook.env_key_dependencies()
            && hook.language_request.satisfied_by(self)
    }

    /// Returns whether this installed environment matches the normalized hook
    /// environment key.
    ///
    /// Used when only the normalized key is available, such as cache GC scanning
    /// configured hooks without building full hook instances.
    pub(crate) fn matches_env_key(&self, key: &HookEnvKey) -> bool {
        self.language == key.language
            && self.dependencies == key.dependencies
            && key.language_request.satisfied_by(self)
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

    use crate::config::{
        Config, HookOptions, Language, PassFilenames, PythonUvLockMode, PythonUvOptions,
        RemoteHook, Shell, Stage, Stages,
    };
    use crate::hook::HookSpec;
    use crate::languages::version::LanguageRequest;
    use crate::workspace::Project;

    use super::{Hook, HookBuilder, Repo};

    #[tokio::test]
    async fn hook_builder_build_fills_and_merges_attributes() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);

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

        // Base hook spec (e.g. from a manifest): options that config can merge or override.
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
            priority: Some(42),
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
                    auto_update: None,
                    repos: [],
                    default_install_hook_types: None,
                    default_language_version: Some(
                        {
                            Python: "python3.12",
                        },
                    ),
                    default_stages: Some(
                        Stages(manual),
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
            env_spec: Dependencies(
                {},
            ),
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
            pass_filenames: None,
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
            stages: Stages(manual),
            verbose: true,
            minimum_prek_version: None,
            priority: 42,
        }
        "#);

        Ok(())
    }

    #[tokio::test]
    async fn hook_builder_empty_hook_stages_inherit_default_stages() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\ndefault_stages: [manual]\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local { hooks: vec![] });

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            priority: None,
            options: HookOptions {
                stages: Some(Stages::from([])),
                ..Default::default()
            },
        };

        let hook = HookBuilder::new(project, repo, hook_spec, 0)
            .build()
            .await?;

        assert_eq!(hook.stages, Stages::from([Stage::Manual]));
        Ok(())
    }

    #[test]
    fn hook_spec_apply_project_defaults_sets_explicit_all_when_default_stages_missing() {
        let config: Config = serde_saphyr::from_str("repos: []\n").expect("config should parse");

        let mut hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            priority: None,
            options: HookOptions::default(),
        };

        hook_spec.apply_project_defaults(&config);

        assert_eq!(hook_spec.options.stages, Some(Stages::ALL));
    }

    #[tokio::test]
    async fn hook_builder_preserves_explicit_empty_default_stages() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\ndefault_stages: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local { hooks: vec![] });

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            priority: None,
            options: HookOptions::default(),
        };

        let hook = HookBuilder::new(project, repo, hook_spec, 0)
            .build()
            .await?;

        assert_eq!(hook.stages, Stages::from([]));
        Ok(())
    }

    #[tokio::test]
    async fn hook_builder_defaults_to_all_when_stages_and_default_stages_missing() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local { hooks: vec![] });

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            priority: None,
            options: HookOptions::default(),
        };

        let hook = HookBuilder::new(project, repo, hook_spec, 0)
            .build()
            .await?;

        assert_eq!(hook.stages, Stages::ALL);
        Ok(())
    }

    #[tokio::test]
    async fn hook_builder_empty_hook_stages_default_to_all_when_default_stages_missing()
    -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local { hooks: vec![] });

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python3 -c 'print(1)'".to_string(),
            language: Language::Python,
            priority: None,
            options: HookOptions {
                stages: Some(Stages::from([])),
                ..Default::default()
            },
        };

        let hook = HookBuilder::new(project, repo, hook_spec, 0)
            .build()
            .await?;

        assert_eq!(hook.stages, Stages::ALL);
        Ok(())
    }

    #[tokio::test]
    async fn hook_builder_rejects_uv_options_for_non_python_uv_language() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local { hooks: vec![] });

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "python -c 'print(1)'".to_string(),
            language: Language::Python,
            priority: None,
            options: HookOptions {
                uv: Some(PythonUvOptions::default()),
                ..Default::default()
            },
        };

        let err = HookBuilder::new(project, repo, hook_spec, 0)
            .build()
            .await
            .unwrap_err();

        let super::Error::Hook { error, .. } = err else {
            panic!("expected hook error");
        };
        assert_eq!(
            error.to_string(),
            "Hook specified `uv` options but the language `python` is not `python_uv`"
        );

        Ok(())
    }

    #[tokio::test]
    async fn hook_builder_rejects_python_uv_additional_dependencies() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local { hooks: vec![] });

        let hook_spec = HookSpec {
            id: "test-hook".to_string(),
            name: "test-hook".to_string(),
            entry: "ty check .".to_string(),
            language: Language::PythonUv,
            priority: None,
            options: HookOptions {
                additional_dependencies: Some(vec!["ty".to_string()]),
                ..Default::default()
            },
        };

        let err = HookBuilder::new(project, repo, hook_spec, 0)
            .build()
            .await
            .unwrap_err();

        let super::Error::Hook { error, .. } = err else {
            panic!("expected hook error");
        };
        assert_eq!(
            error.to_string(),
            "`language: python_uv` does not install `additional_dependencies`; add Python packages to a uv dependency group and update `uv.lock` instead"
        );

        Ok(())
    }

    #[tokio::test]
    async fn hook_builder_python_uv_env_key_tracks_lockfile_content() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let config_path = temp.path().join(PRE_COMMIT_CONFIG_YAML);
        fs_err::write(&config_path, "repos: []\n")?;
        fs_err::write(
            temp.path().join("pyproject.toml"),
            "[project]\nname = \"example\"\nversion = \"0.1.0\"\n",
        )?;
        fs_err::write(temp.path().join("uv.lock"), "version = 1\n")?;

        let project = Arc::new(Project::from_config_file(
            Cow::Borrowed(&config_path),
            None,
        )?);
        let repo = Arc::new(Repo::Local { hooks: vec![] });

        let hook_spec = HookSpec {
            id: "ty".to_string(),
            name: "ty".to_string(),
            entry: "ty check .".to_string(),
            language: Language::PythonUv,
            priority: None,
            options: HookOptions {
                uv: Some(PythonUvOptions {
                    dependency_groups: Some(vec![
                        "typecheck".to_string(),
                        "lint".to_string(),
                        "typecheck".to_string(),
                    ]),
                    extras: Some(vec!["typed".to_string(), "cli".to_string()]),
                    install_project: Some(false),
                    lock_mode: Some(PythonUvLockMode::Frozen),
                    ..Default::default()
                }),
                ..Default::default()
            },
        };

        let hook = HookBuilder::new(project.clone(), repo.clone(), hook_spec.clone(), 0)
            .build()
            .await?;
        let uv = hook.python_uv_env().expect("python_uv env");
        assert_eq!(uv.dependency_groups, ["lint", "typecheck"]);
        assert_eq!(uv.extras, ["cli", "typed"]);
        assert!(!uv.install_project);
        assert_eq!(uv.lock_mode, PythonUvLockMode::Frozen);
        let first_dependencies = hook.env_key_dependencies().clone();

        fs_err::write(temp.path().join("uv.lock"), "version = 2\n")?;

        let hook = HookBuilder::new(project, repo, hook_spec, 0)
            .build()
            .await?;
        assert_ne!(first_dependencies, *hook.env_key_dependencies());

        Ok(())
    }

    /// Set up a temporary directory with a minimal `.pre-commit-config.yaml`
    /// and a `remote-repo` subdirectory.
    fn setup_python_hook_test() -> Result<(tempfile::TempDir, Arc<Project>)> {
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

    /// Build a hook from the given repo path and options via `HookBuilder`.
    async fn build_python_hook(
        project: Arc<Project>,
        repo_path: PathBuf,
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
            entry: "./hook.py".to_string(),
            language: Language::Python,
            priority: None,
            options: HookOptions {
                language_version: language_version.map(str::to_string),
                ..Default::default()
            },
        };

        Ok(HookBuilder::new(project, repo, hook_spec, 0)
            .build()
            .await?)
    }

    static PEP723_SCRIPT: &str = indoc::indoc! {r#"
        # /// script
        # requires-python = ">=3.11"
        # ///
        print("hello")
    "#};

    static PEP723_SCRIPT_WITH_DEPENDENCIES: &str = indoc::indoc! {r#"
        # /// script
        # dependencies = ["pyecho-cli"]
        # ///
        print("hello")
    "#};

    #[tokio::test]
    async fn hook_builder_python_pep723_overrides_user_and_pyproject() -> Result<()> {
        let (temp, project) = setup_python_hook_test()?;
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
    async fn hook_builder_python_pep723_dependencies_are_env_key_dependencies() -> Result<()> {
        let (temp, project) = setup_python_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(repo_path.join("hook.py"), PEP723_SCRIPT_WITH_DEPENDENCIES)?;

        let hook = build_python_hook(project, repo_path, None).await?;

        assert!(hook.additional_dependencies.contains("pyecho-cli"));
        assert!(hook.env_key_dependencies().contains("pyecho-cli"));
        Ok(())
    }

    #[tokio::test]
    async fn hook_builder_python_user_language_version_overrides_pyproject() -> Result<()> {
        let (temp, project) = setup_python_hook_test()?;
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
    async fn hook_builder_python_pep723_overrides_pyproject_without_user_version() -> Result<()> {
        let (temp, project) = setup_python_hook_test()?;
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
    async fn hook_builder_python_defaults_to_any_without_version_sources() -> Result<()> {
        let (temp, project) = setup_python_hook_test()?;
        let repo_path = temp.path().join("remote-repo");
        fs_err::write(repo_path.join("hook.py"), "print(\"hello\")\n")?;

        let hook = build_python_hook(project, repo_path, None).await?;

        assert!(hook.language_request.is_any());
        Ok(())
    }

    #[tokio::test]
    async fn hook_builder_python_pyproject_provides_version_when_no_other_source() -> Result<()> {
        let (temp, project) = setup_python_hook_test()?;
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
}
