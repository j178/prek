use std::collections::BTreeMap;
use std::error::Error as _;
use std::fmt::Display;
use std::ops::RangeInclusive;
use std::path::Path;

use anyhow::Result;
use fancy_regex::Regex;
use globset::{Glob, GlobSet, GlobSetBuilder};
use itertools::Itertools;
use rustc_hash::FxHashMap;
use serde::de::{Error as DeError, MapAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};

use crate::fs::Simplified;
use crate::identify;
use crate::install_source::InstallSource;
#[cfg(feature = "schemars")]
use crate::schema::{schema_repo_builtin, schema_repo_local, schema_repo_meta, schema_repo_remote};
use crate::version;
use crate::warn_user;
use crate::warn_user_once;

#[derive(Clone)]
pub(crate) struct GlobPatterns {
    patterns: Vec<String>,
    set: GlobSet,
}

impl GlobPatterns {
    pub(crate) fn new(patterns: Vec<String>) -> Result<Self, globset::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &patterns {
            builder.add(Glob::new(pattern)?);
        }
        let set = builder.build()?;
        Ok(Self { patterns, set })
    }

    pub(crate) fn patterns(&self) -> &[String] {
        &self.patterns
    }

    fn is_match(&self, value: &str) -> bool {
        self.set.is_match(Path::new(value))
    }
}

impl std::fmt::Debug for GlobPatterns {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GlobPatterns")
            .field("patterns", &self.patterns)
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum FilePatternWire {
    Glob { glob: String },
    GlobList { glob: Vec<String> },
    Regex(String),
}

#[derive(Debug, thiserror::Error)]
enum FilePatternWireError {
    #[error(transparent)]
    Glob(#[from] globset::Error),

    #[error(transparent)]
    Regex(#[from] fancy_regex::Error),
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "FilePatternWire")]
pub(crate) enum FilePattern {
    Regex(Regex),
    Glob(GlobPatterns),
}

impl FilePattern {
    pub(crate) fn new_glob(patterns: Vec<String>) -> Result<Self, globset::Error> {
        Ok(Self::Glob(GlobPatterns::new(patterns)?))
    }

    pub(crate) fn new_regex(pattern: &str) -> Result<Self, fancy_regex::Error> {
        Ok(Self::Regex(Regex::new(pattern)?))
    }

    pub(crate) fn is_match(&self, str: &str) -> bool {
        match self {
            FilePattern::Regex(regex) => regex.is_match(str).unwrap_or(false),
            FilePattern::Glob(globs) => globs.is_match(str),
        }
    }

    pub(crate) fn regex_pattern(&self) -> Option<&str> {
        match self {
            FilePattern::Regex(regex) => Some(regex.as_str()),
            FilePattern::Glob(_) => None,
        }
    }

    pub(crate) fn glob_patterns(&self) -> Option<&[String]> {
        match self {
            FilePattern::Regex(_) => None,
            FilePattern::Glob(globs) => Some(globs.patterns()),
        }
    }
}

impl Display for FilePattern {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FilePattern::Regex(regex) => write!(f, "regex: {}", regex.as_str()),
            FilePattern::Glob(globs) => {
                let patterns = globs.patterns.iter().join(", ");
                write!(f, "glob: [{patterns}]")
            }
        }
    }
}

impl TryFrom<FilePatternWire> for FilePattern {
    type Error = FilePatternWireError;

    fn try_from(value: FilePatternWire) -> Result<Self, Self::Error> {
        match value {
            FilePatternWire::Glob { glob } => Ok(Self::Glob(GlobPatterns::new(vec![glob])?)),
            FilePatternWire::GlobList { glob } => Ok(Self::Glob(GlobPatterns::new(glob)?)),
            FilePatternWire::Regex(pattern) => Ok(Self::Regex(Regex::new(&pattern)?)),
        }
    }
}

#[derive(
    Debug,
    Copy,
    Clone,
    PartialEq,
    Eq,
    Hash,
    Deserialize,
    Serialize,
    clap::ValueEnum,
    strum::AsRefStr,
    strum::Display,
)]
#[serde(rename_all = "snake_case")]
#[strum(serialize_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[non_exhaustive]
pub enum Language {
    Bun,
    Conda,
    Coursier,
    Dart,
    Docker,
    DockerImage,
    Dotnet,
    Fail,
    Golang,
    Haskell,
    Julia,
    Lua,
    Node,
    Perl,
    Pygrep,
    Python,
    R,
    Ruby,
    Rust,
    #[serde(alias = "unsupported_script")]
    Script,
    Swift,
    #[serde(alias = "unsupported")]
    System,
}

#[derive(
    Debug, Clone, Copy, Default, Deserialize, clap::ValueEnum, strum::AsRefStr, strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) enum HookType {
    CommitMsg,
    PostCheckout,
    PostCommit,
    PostMerge,
    PostRewrite,
    #[default]
    PreCommit,
    PreMergeCommit,
    PrePush,
    PreRebase,
    PrepareCommitMsg,
}

impl HookType {
    /// Return the number of arguments this hook type expects.
    pub fn num_args(self) -> RangeInclusive<usize> {
        match self {
            Self::CommitMsg => 1..=1,
            Self::PostCheckout => 3..=3,
            Self::PreCommit => 0..=0,
            Self::PostCommit => 0..=0,
            Self::PreMergeCommit => 0..=0,
            Self::PostMerge => 1..=1,
            Self::PostRewrite => 1..=1,
            Self::PrePush => 2..=2,
            Self::PreRebase => 1..=2,
            Self::PrepareCommitMsg => 1..=3,
        }
    }
}

#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Default,
    Hash,
    Deserialize,
    Serialize,
    clap::ValueEnum,
    strum::AsRefStr,
    strum::Display,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) enum Stage {
    Manual,
    CommitMsg,
    PostCheckout,
    PostCommit,
    PostMerge,
    PostRewrite,
    #[default]
    #[serde(alias = "commit")]
    PreCommit,
    #[serde(alias = "merge-commit")]
    PreMergeCommit,
    #[serde(alias = "push")]
    PrePush,
    PreRebase,
    PrepareCommitMsg,
}

impl From<HookType> for Stage {
    fn from(value: HookType) -> Self {
        match value {
            HookType::CommitMsg => Self::CommitMsg,
            HookType::PostCheckout => Self::PostCheckout,
            HookType::PostCommit => Self::PostCommit,
            HookType::PostMerge => Self::PostMerge,
            HookType::PostRewrite => Self::PostRewrite,
            HookType::PreCommit => Self::PreCommit,
            HookType::PreMergeCommit => Self::PreMergeCommit,
            HookType::PrePush => Self::PrePush,
            HookType::PreRebase => Self::PreRebase,
            HookType::PrepareCommitMsg => Self::PrepareCommitMsg,
        }
    }
}

impl Stage {
    pub fn operate_on_files(self) -> bool {
        matches!(
            self,
            Stage::Manual
                | Stage::CommitMsg
                | Stage::PreCommit
                | Stage::PreMergeCommit
                | Stage::PrePush
                | Stage::PrepareCommitMsg
        )
    }
}

/// Common hook options.
#[derive(Debug, Clone, Default, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct HookOptions {
    /// Not documented in the official docs.
    pub alias: Option<String>,
    /// The pattern of files to run on.
    pub files: Option<FilePattern>,
    /// Exclude files that were matched by `files`.
    /// Default is `$^`, which matches nothing.
    pub exclude: Option<FilePattern>,
    /// List of file types to run on (AND).
    /// Default is `[file]`, which matches all files.
    #[serde(deserialize_with = "deserialize_and_validate_tags", default)]
    pub types: Option<Vec<String>>,
    /// List of file types to run on (OR).
    /// Default is `[]`.
    #[serde(deserialize_with = "deserialize_and_validate_tags", default)]
    pub types_or: Option<Vec<String>>,
    /// List of file types to exclude.
    /// Default is `[]`.
    #[serde(deserialize_with = "deserialize_and_validate_tags", default)]
    pub exclude_types: Option<Vec<String>>,
    /// Not documented in the official docs.
    pub additional_dependencies: Option<Vec<String>>,
    /// Additional arguments to pass to the hook.
    pub args: Option<Vec<String>>,
    /// Environment variables to set for the hook.
    pub env: Option<FxHashMap<String, String>>,
    /// This hook will run even if there are no matching files.
    /// Default is false.
    pub always_run: Option<bool>,
    /// If this hook fails, don't run any more hooks.
    /// Default is false.
    pub fail_fast: Option<bool>,
    /// Append filenames that would be checked to the hook entry as arguments.
    /// Default is true.
    pub pass_filenames: Option<bool>,
    /// A description of the hook. For metadata only.
    pub description: Option<String>,
    /// Run the hook on a specific version of the language.
    /// Default is `default`.
    /// See <https://pre-commit.com/#overriding-language-version>.
    pub language_version: Option<String>,
    /// Write the output of the hook to a file when the hook fails or verbose is enabled.
    pub log_file: Option<String>,
    /// This hook will execute using a single process instead of in parallel.
    /// Default is false.
    pub require_serial: Option<bool>,
    /// Select which git hook(s) to run for.
    /// Default all stages are selected.
    /// See <https://pre-commit.com/#confining-hooks-to-run-at-certain-stages>.
    pub stages: Option<Vec<Stage>>,
    /// Print the output of the hook even if it passes.
    /// Default is false.
    pub verbose: Option<bool>,
    /// The minimum version of prek required to run this hook.
    #[serde(deserialize_with = "deserialize_and_validate_minimum_version", default)]
    pub minimum_prek_version: Option<String>,

    #[serde(skip_serializing, flatten)]
    pub _unused_keys: BTreeMap<String, serde_json::Value>,
}

impl HookOptions {
    pub fn update(&mut self, other: &Self) {
        macro_rules! update_if_some {
            ($($field:ident),* $(,)?) => {
                $(
                if other.$field.is_some() {
                    self.$field.clone_from(&other.$field);
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
            minimum_prek_version,
        );

        // Merge environment variables.
        if let Some(other_env) = &other.env {
            if let Some(self_env) = &mut self.env {
                self_env.extend(other_env.clone());
            } else {
                self.env.clone_from(&other.env);
            }
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct ManifestHook {
    /// The id of the hook.
    pub id: String,
    /// The name of the hook.
    pub name: String,
    /// The command to run. It can contain arguments that will not be overridden.
    pub entry: String,
    /// The language of the hook. Tells prek how to install and run the hook.
    pub language: Language,
    #[serde(flatten)]
    pub options: HookOptions,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(transparent)]
pub(crate) struct Manifest {
    pub hooks: Vec<ManifestHook>,
}

/// A remote hook in the configuration file.
///
/// All keys in manifest hook dict are valid in a config hook dict, but are optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct RemoteHook {
    /// The id of the hook.
    pub id: String,
    /// Override the name of the hook.
    pub name: Option<String>,
    /// Override the entrypoint. Not documented in the official docs but works.
    pub entry: Option<String>,
    /// Override the language. Not documented in the official docs but works.
    pub language: Option<Language>,
    /// Priority used by the scheduler to determine ordering and concurrency.
    /// Hooks with the same priority can run in parallel.
    ///
    /// This is only allowed in project config files (e.g. `.pre-commit-config.yaml`).
    /// It is not allowed in manifests (e.g. `.pre-commit-hooks.yaml`).
    pub priority: Option<u32>,
    #[serde(flatten)]
    pub options: HookOptions,
}

/// A local hook in the configuration file.
///
/// This is similar to `ManifestHook`, but includes config-only fields (like `priority`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct LocalHook {
    /// The id of the hook.
    pub id: String,
    /// The name of the hook.
    pub name: String,
    /// The command to run. It can contain arguments that will not be overridden.
    pub entry: String,
    /// The language of the hook. Tells prek how to install and run the hook.
    pub language: Language,
    /// Priority used by the scheduler to determine ordering and concurrency.
    /// Hooks with the same priority can run in parallel.
    pub priority: Option<u32>,
    #[serde(flatten)]
    pub options: HookOptions,
}

/// A meta hook predefined in pre-commit.
///
/// It's the same as the manifest hook definition but with only a few predefined id allowed.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(try_from = "RemoteHook")]
pub(crate) struct MetaHook {
    /// The id of the hook.
    pub id: String,
    /// The name of the hook.
    pub name: String,
    /// Priority used by the scheduler to determine ordering and concurrency.
    /// Hooks with the same priority can run in parallel.
    pub priority: Option<u32>,
    #[serde(flatten)]
    pub options: HookOptions,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PredefinedHookWireError {
    #[error("unknown {kind} hook id `{id}`")]
    UnknownId {
        kind: PredefinedHookKind,
        id: String,
    },

    #[error("language must be `system` for {kind} hooks")]
    InvalidLanguage { kind: PredefinedHookKind },

    #[error("`entry` is not allowed for {kind} hooks")]
    EntryNotAllowed { kind: PredefinedHookKind },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PredefinedHookKind {
    Meta,
    Builtin,
}

impl Display for PredefinedHookKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Meta => f.write_str("meta"),
            Self::Builtin => f.write_str("builtin"),
        }
    }
}

impl TryFrom<RemoteHook> for MetaHook {
    type Error = PredefinedHookWireError;

    fn try_from(hook_options: RemoteHook) -> Result<Self, Self::Error> {
        let mut meta_hook = MetaHook::from_id(&hook_options.id).map_err(|()| {
            PredefinedHookWireError::UnknownId {
                kind: PredefinedHookKind::Meta,
                id: hook_options.id.clone(),
            }
        })?;

        if hook_options.language.is_some_and(|l| l != Language::System) {
            return Err(PredefinedHookWireError::InvalidLanguage {
                kind: PredefinedHookKind::Meta,
            });
        }
        if hook_options.entry.is_some() {
            return Err(PredefinedHookWireError::EntryNotAllowed {
                kind: PredefinedHookKind::Meta,
            });
        }

        if let Some(name) = &hook_options.name {
            meta_hook.name.clone_from(name);
        }
        if hook_options.priority.is_some() {
            meta_hook.priority = hook_options.priority;
        }
        meta_hook.options.update(&hook_options.options);

        Ok(meta_hook)
    }
}

/// A builtin hook predefined in prek.
/// Basically the same as meta hooks, but defined under `builtin` repo, and do other non-meta checks.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RemoteHook")]
pub(crate) struct BuiltinHook {
    /// The id of the hook.
    pub id: String,
    /// The name of the hook.
    ///
    /// This is populated from the predefined builtin hook definition.
    pub name: String,
    /// The command to run. It can contain arguments that will not be overridden.
    pub entry: String,
    /// Priority used by the scheduler to determine ordering and concurrency.
    /// Hooks with the same priority can run in parallel.
    pub priority: Option<u32>,
    /// Common hook options.
    ///
    /// Builtin hooks allow the same set of options overrides as other hooks.
    #[serde(flatten)]
    pub options: HookOptions,
}

impl TryFrom<RemoteHook> for BuiltinHook {
    type Error = PredefinedHookWireError;

    fn try_from(hook_options: RemoteHook) -> Result<Self, Self::Error> {
        let mut builtin_hook = BuiltinHook::from_id(&hook_options.id).map_err(|()| {
            PredefinedHookWireError::UnknownId {
                kind: PredefinedHookKind::Builtin,
                id: hook_options.id.clone(),
            }
        })?;

        if hook_options.language.is_some_and(|l| l != Language::System) {
            return Err(PredefinedHookWireError::InvalidLanguage {
                kind: PredefinedHookKind::Builtin,
            });
        }
        if hook_options.entry.is_some() {
            return Err(PredefinedHookWireError::EntryNotAllowed {
                kind: PredefinedHookKind::Builtin,
            });
        }

        if let Some(name) = &hook_options.name {
            builtin_hook.name.clone_from(name);
        }
        if hook_options.priority.is_some() {
            builtin_hook.priority = hook_options.priority;
        }
        builtin_hook.options.update(&hook_options.options);

        Ok(builtin_hook)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct RemoteRepo {
    #[cfg_attr(feature = "schemars", schemars(schema_with = "schema_repo_remote"))]
    pub repo: String,
    pub rev: String,
    #[serde(skip_serializing)]
    pub hooks: Vec<RemoteHook>,

    #[serde(skip_serializing, flatten)]
    _unused_keys: BTreeMap<String, serde_json::Value>,
}

impl RemoteRepo {
    pub fn new(repo: String, rev: String, hooks: Vec<RemoteHook>) -> Self {
        Self {
            repo,
            rev,
            hooks,
            _unused_keys: BTreeMap::new(),
        }
    }
}

impl PartialEq for RemoteRepo {
    fn eq(&self, other: &Self) -> bool {
        self.repo == other.repo && self.rev == other.rev
    }
}

impl Eq for RemoteRepo {}

impl std::hash::Hash for RemoteRepo {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.repo.hash(state);
        self.rev.hash(state);
    }
}

impl Display for RemoteRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.repo, self.rev)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct LocalRepo {
    #[cfg_attr(feature = "schemars", schemars(schema_with = "schema_repo_local"))]
    pub repo: String,
    pub hooks: Vec<LocalHook>,

    #[serde(skip_serializing, flatten)]
    _unused_keys: BTreeMap<String, serde_json::Value>,
}

impl Display for LocalRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("local")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct MetaRepo {
    #[cfg_attr(feature = "schemars", schemars(schema_with = "schema_repo_meta"))]
    pub repo: String,
    pub hooks: Vec<MetaHook>,

    #[serde(skip_serializing, flatten)]
    _unused_keys: BTreeMap<String, serde_json::Value>,
}

impl Display for MetaRepo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("meta")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct BuiltinRepo {
    #[cfg_attr(feature = "schemars", schemars(schema_with = "schema_repo_builtin"))]
    pub repo: String,
    pub hooks: Vec<BuiltinHook>,

    #[serde(skip_serializing, flatten)]
    _unused_keys: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub(crate) enum Repo {
    Remote(RemoteRepo),
    Local(LocalRepo),
    Meta(MetaRepo),
    Builtin(BuiltinRepo),
}

impl<'de> Deserialize<'de> for Repo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RepoVisitor;

        impl<'de> Visitor<'de> for RepoVisitor {
            type Value = Repo;

            fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
                formatter.write_str("a repo mapping")
            }

            fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
            where
                M: MapAccess<'de>,
            {
                enum HooksValue {
                    Remote(Vec<RemoteHook>),
                    Local(Vec<LocalHook>),
                    Meta(Vec<MetaHook>),
                    Builtin(Vec<BuiltinHook>),
                }

                let mut repo: Option<String> = None;
                let mut rev: Option<String> = None;
                let mut hooks: Option<HooksValue> = None;
                let mut unused = BTreeMap::new();

                while let Some(key) = map.next_key::<String>()? {
                    match key.as_str() {
                        "repo" => {
                            let repo_value: String = map.next_value()?;
                            repo = Some(repo_value);
                        }
                        "rev" => {
                            rev = Some(map.next_value()?);
                        }
                        "hooks" => {
                            hooks = Some(match repo.as_deref() {
                                Some("local") => HooksValue::Local(map.next_value()?),
                                Some("meta") => HooksValue::Meta(map.next_value()?),
                                Some("builtin") => HooksValue::Builtin(map.next_value()?),
                                // Not seen `repo` yet, assume remote.
                                _ => HooksValue::Remote(map.next_value()?),
                            });
                        }
                        _ => {
                            let value = map.next_value::<serde_json::Value>()?;
                            unused.insert(key, value);
                        }
                    }
                }

                let repo_value = repo.ok_or_else(|| M::Error::missing_field("repo"))?;
                match repo_value.as_str() {
                    "local" => {
                        if rev.is_some() {
                            return Err(M::Error::custom("`rev` is not allowed for local repos"));
                        }
                        let hooks = match hooks.ok_or_else(|| M::Error::missing_field("hooks"))? {
                            HooksValue::Local(hooks) => hooks,
                            HooksValue::Remote(hooks) => hooks
                                .into_iter()
                                .map(remote_hook_to_local::<M::Error>)
                                .collect::<Result<Vec<_>, _>>()?,
                            HooksValue::Meta(_) | HooksValue::Builtin(_) => {
                                return Err(M::Error::custom("invalid hooks for local repo"));
                            }
                        };
                        Ok(Repo::Local(LocalRepo {
                            repo: "local".to_string(),
                            hooks,
                            _unused_keys: unused,
                        }))
                    }
                    "meta" => {
                        if rev.is_some() {
                            return Err(M::Error::custom("`rev` is not allowed for meta repos"));
                        }
                        let hooks = match hooks.ok_or_else(|| M::Error::missing_field("hooks"))? {
                            HooksValue::Meta(hooks) => hooks,
                            HooksValue::Remote(hooks) => hooks
                                .into_iter()
                                .map(|hook| MetaHook::try_from(hook).map_err(M::Error::custom))
                                .collect::<Result<Vec<_>, _>>()?,
                            HooksValue::Local(_) | HooksValue::Builtin(_) => {
                                return Err(M::Error::custom("invalid hooks for meta repo"));
                            }
                        };
                        Ok(Repo::Meta(MetaRepo {
                            repo: "meta".to_string(),
                            hooks,
                            _unused_keys: unused,
                        }))
                    }
                    "builtin" => {
                        if rev.is_some() {
                            return Err(M::Error::custom("`rev` is not allowed for builtin repos"));
                        }
                        let hooks = match hooks.ok_or_else(|| M::Error::missing_field("hooks"))? {
                            HooksValue::Builtin(hooks) => hooks,
                            HooksValue::Remote(hooks) => hooks
                                .into_iter()
                                .map(|hook| BuiltinHook::try_from(hook).map_err(M::Error::custom))
                                .collect::<Result<Vec<_>, _>>()?,
                            HooksValue::Local(_) | HooksValue::Meta(_) => {
                                return Err(M::Error::custom("invalid hooks for builtin repo"));
                            }
                        };
                        Ok(Repo::Builtin(BuiltinRepo {
                            repo: "builtin".to_string(),
                            hooks,
                            _unused_keys: unused,
                        }))
                    }
                    _ => {
                        let rev = rev.ok_or_else(|| M::Error::missing_field("rev"))?;
                        let hooks = match hooks.ok_or_else(|| M::Error::missing_field("hooks"))? {
                            HooksValue::Remote(hooks) => hooks,
                            HooksValue::Local(_) | HooksValue::Meta(_) | HooksValue::Builtin(_) => {
                                return Err(M::Error::custom("invalid hooks for remote repo"));
                            }
                        };
                        Ok(Repo::Remote(RemoteRepo {
                            repo: repo_value,
                            rev,
                            hooks,
                            _unused_keys: unused,
                        }))
                    }
                }
            }
        }

        deserializer.deserialize_map(RepoVisitor)
    }
}

fn remote_hook_to_local<E>(hook: RemoteHook) -> Result<LocalHook, E>
where
    E: DeError,
{
    Ok(LocalHook {
        id: hook.id,
        name: hook.name.ok_or_else(|| E::missing_field("name"))?,
        entry: hook.entry.ok_or_else(|| E::missing_field("entry"))?,
        language: hook.language.ok_or_else(|| E::missing_field("language"))?,
        priority: hook.priority,
        options: hook.options,
    })
}

// TODO: warn sensible regex
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(
    feature = "schemars",
    schemars(title = "prek configuration file format")
)]
pub(crate) struct Config {
    pub repos: Vec<Repo>,
    /// A list of `--hook-types` which will be used by default when running `prek install`.
    /// Default is `[pre-commit]`.
    pub default_install_hook_types: Option<Vec<HookType>>,
    /// A mapping from language to the default `language_version`.
    pub default_language_version: Option<FxHashMap<Language, String>>,
    /// A configuration-wide default for the stages property of hooks.
    /// Default to all stages.
    pub default_stages: Option<Vec<Stage>>,
    /// Global file include pattern.
    pub files: Option<FilePattern>,
    /// Global file exclude pattern.
    pub exclude: Option<FilePattern>,
    /// Set to true to have prek stop running hooks after the first failure.
    /// Default is false.
    pub fail_fast: Option<bool>,
    /// The minimum version of prek required to run this configuration.
    #[serde(deserialize_with = "deserialize_and_validate_minimum_version", default)]
    pub minimum_prek_version: Option<String>,
    /// Set to true to isolate this project from parent configurations in workspace mode.
    /// When true, files in this project are "consumed" by this project and will not be processed
    /// by parent projects.
    /// When false (default), files in subprojects are processed by both the subproject and
    /// any parent projects that contain them.
    pub orphan: Option<bool>,

    #[serde(skip_serializing, flatten)]
    _unused_keys: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("Failed to parse `{0}`")]
    Yaml(String, #[source] Box<serde_saphyr::Error>),

    #[error("Failed to parse `{0}`")]
    Toml(String, #[source] Box<toml::de::Error>),
}

impl Error {
    /// Warn the user if the config error is a parse error (not "file not found").
    pub(crate) fn warn_parse_error(&self) {
        // Skip file not found errors.
        if matches!(self, Self::Io(e) if e.kind() == std::io::ErrorKind::NotFound) {
            return;
        }
        if let Some(cause) = self.source() {
            warn_user_once!("{self}: {cause}");
        } else {
            warn_user_once!("{self}");
        }
    }
}

/// Keys that prek does not use.
const EXPECTED_UNUSED: &[&str] = &["minimum_pre_commit_version", "ci"];

fn push_unused_paths<'a, I>(acc: &mut Vec<String>, prefix: &str, keys: I)
where
    I: Iterator<Item = &'a str>,
{
    for key in keys {
        let path = if prefix.is_empty() {
            key.to_string()
        } else {
            format!("{prefix}.{key}")
        };
        acc.push(path);
    }
}

fn collect_unused_paths(config: &Config) -> Vec<String> {
    let mut paths = Vec::new();

    push_unused_paths(
        &mut paths,
        "",
        config._unused_keys.keys().filter_map(|key| {
            let key = key.as_str();
            (!EXPECTED_UNUSED.contains(&key)).then_some(key)
        }),
    );

    for (repo_idx, repo) in config.repos.iter().enumerate() {
        let repo_prefix = format!("repos[{repo_idx}]");
        let (repo_unused_keys, hooks_options): (_, Box<dyn Iterator<Item = &HookOptions>>) =
            match repo {
                Repo::Remote(remote) => (
                    &remote._unused_keys,
                    Box::new(remote.hooks.iter().map(|h| &h.options)),
                ),
                Repo::Local(local) => (
                    &local._unused_keys,
                    Box::new(local.hooks.iter().map(|h| &h.options)),
                ),
                Repo::Meta(meta) => (
                    &meta._unused_keys,
                    Box::new(meta.hooks.iter().map(|h| &h.options)),
                ),
                Repo::Builtin(builtin) => (
                    &builtin._unused_keys,
                    Box::new(builtin.hooks.iter().map(|h| &h.options)),
                ),
            };

        push_unused_paths(
            &mut paths,
            &repo_prefix,
            repo_unused_keys.keys().map(String::as_str),
        );
        for (hook_idx, options) in hooks_options.enumerate() {
            let hook_prefix = format!("{repo_prefix}.hooks[{hook_idx}]");
            push_unused_paths(
                &mut paths,
                &hook_prefix,
                options._unused_keys.keys().map(String::as_str),
            );
        }
    }

    paths
}

fn warn_unused_paths(path: &Path, entries: &[String]) {
    if entries.is_empty() {
        return;
    }

    if entries.len() < 4 {
        let inline = entries
            .iter()
            .map(|entry| format!("`{}`", entry.yellow()))
            .join(", ");
        warn_user!(
            "Ignored unexpected keys in `{}`: {inline}",
            path.user_display().cyan()
        );
    } else {
        let list = entries
            .iter()
            .map(|entry| format!("  - `{}`", entry.yellow()))
            .join("\n");
        warn_user!(
            "Ignored unexpected keys in `{}`:\n{list}",
            path.user_display().cyan()
        );
    }
}

/// Read the configuration file from the given path.
pub(crate) fn load_config(path: &Path) -> Result<Config, Error> {
    let content = fs_err::read_to_string(path)?;

    let config = match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("toml") => toml::from_str(&content)
            .map_err(|e| Error::Toml(path.user_display().to_string(), Box::new(e)))?,
        _ => serde_saphyr::from_str(&content)
            .map_err(|e| Error::Yaml(path.user_display().to_string(), Box::new(e)))?,
    };

    Ok(config)
}

/// Read the configuration file from the given path, and warn about certain issues.
pub(crate) fn read_config(path: &Path) -> Result<Config, Error> {
    let config = load_config(path)?;

    let unused_paths = collect_unused_paths(&config);
    warn_unused_paths(path, &unused_paths);

    // Check for mutable revs and warn the user.
    let repos_has_mutable_rev = config
        .repos
        .iter()
        .filter_map(|repo| {
            if let Repo::Remote(repo) = repo {
                let rev = &repo.rev;
                // A rev is considered mutable if it doesn't contain a '.' (like a version)
                // and is not a hexadecimal string (like a commit SHA).
                if !rev.contains('.') && !looks_like_sha(rev) {
                    return Some(repo);
                }
            }
            None
        })
        .collect::<Vec<_>>();
    if !repos_has_mutable_rev.is_empty() {
        let msg = repos_has_mutable_rev
            .iter()
            .map(|repo| format!("{}: {}", repo.repo.cyan(), repo.rev.yellow()))
            .join("\n");

        warn_user!(
            "{}",
            indoc::formatdoc! { r#"
            The following repos have mutable `rev` fields (moving tag / branch):
            {}
            Mutable references are never updated after first install and are not supported.
            See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.
            hint: `prek auto-update` often fixes this",
            "#,
            msg
            }
        );
    }

    Ok(config)
}

/// Read the manifest file from the given path.
pub(crate) fn read_manifest(path: &Path) -> Result<Manifest, Error> {
    let content = fs_err::read_to_string(path)?;
    let manifest: Manifest = serde_saphyr::from_str(&content)
        .map_err(|e| Error::Yaml(path.user_display().to_string(), Box::new(e)))?;

    Ok(manifest)
}

/// Check if a string looks like a git SHA
fn looks_like_sha(s: &str) -> bool {
    !s.is_empty() && s.as_bytes().iter().all(u8::is_ascii_hexdigit)
}

fn deserialize_and_validate_minimum_version<'de, D>(
    deserializer: D,
) -> Result<Option<String>, D::Error>
where
    D: Deserializer<'de>,
{
    let s = String::deserialize(deserializer)?;
    if s.is_empty() {
        return Ok(None);
    }

    let version = s
        .parse::<semver::Version>()
        .map_err(serde::de::Error::custom)?;
    let cur_version = version::version()
        .version
        .parse::<semver::Version>()
        .expect("Invalid prek version");
    if version > cur_version {
        let hint = InstallSource::detect()
            .map(|s| format!("To update, run `{}`.", s.update_instructions()))
            .unwrap_or("Please consider updating prek".to_string());

        return Err(serde::de::Error::custom(format!(
            "Required minimum prek version `{version}` is greater than current version `{cur_version}`; {hint}",
        )));
    }

    Ok(Some(s))
}

/// Deserializes a vector of strings and validates that each is a known file type tag.
fn deserialize_and_validate_tags<'de, D>(deserializer: D) -> Result<Option<Vec<String>>, D::Error>
where
    D: Deserializer<'de>,
{
    let tags_opt: Option<Vec<String>> = Option::deserialize(deserializer)?;
    if let Some(tags) = &tags_opt {
        let all_tags = identify::all_tags();
        for tag in tags {
            if !all_tags.contains(tag.as_str()) {
                let msg = format!(
                    "Type tag `{tag}` is not recognized. Check for typos or upgrade prek to get new tags."
                );
                return Err(serde::de::Error::custom(msg));
            }
        }
    }
    Ok(tags_opt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Filter to replace dynamic version in snapshots
    const VERSION_FILTER: (&str, &str) = (
        r"current version `\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?`",
        "current version `[CURRENT_VERSION]`",
    );

    #[test]
    fn parse_file_patterns_regex_and_glob() {
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            files: FilePattern,
            exclude: FilePattern,
        }

        let regex_yaml = indoc::indoc! {r"
            files: ^src/
            exclude: ^target/
        "};
        let parsed: Wrapper =
            serde_saphyr::from_str(regex_yaml).expect("regex patterns should parse");
        assert!(
            matches!(parsed.files, FilePattern::Regex(_)),
            "expected regex pattern"
        );
        assert!(parsed.files.is_match("src/main.rs"));
        assert!(!parsed.files.is_match("other/main.rs"));
        assert!(parsed.exclude.is_match("target/debug/app"));

        let glob_yaml = indoc::indoc! {r"
            files:
              glob: src/**/*.rs
            exclude:
              glob: target/**
        "};
        let parsed: Wrapper =
            serde_saphyr::from_str(glob_yaml).expect("glob patterns should parse");
        assert!(
            matches!(parsed.files, FilePattern::Glob(_)),
            "expected glob pattern"
        );
        assert!(parsed.files.is_match("src/lib/main.rs"));
        assert!(!parsed.files.is_match("src/lib/main.py"));
        assert!(parsed.exclude.is_match("target/debug/app"));
        assert!(!parsed.exclude.is_match("src/lib/main.rs"));

        let glob_list_yaml = indoc::indoc! {r"
            files:
              glob:
                - src/**/*.rs
                - crates/**/src/**/*.rs
            exclude:
              glob:
                - target/**
                - dist/**
        "};
        let parsed: Wrapper =
            serde_saphyr::from_str(glob_list_yaml).expect("glob list patterns should parse");
        assert!(parsed.files.is_match("src/lib/main.rs"));
        assert!(parsed.files.is_match("crates/foo/src/lib.rs"));
        assert!(!parsed.files.is_match("tests/main.rs"));
        assert!(parsed.exclude.is_match("target/debug/app"));
        assert!(parsed.exclude.is_match("dist/app"));
    }

    #[test]
    fn file_patterns_expose_sources_and_display() {
        let pattern: FilePattern = serde_saphyr::from_str(indoc::indoc! {r"
            glob:
              - src/**/*.rs
              - crates/**/src/**/*.rs
        "})
        .expect("glob list should parse");
        assert_eq!(
            pattern.to_string(),
            "glob: [src/**/*.rs, crates/**/src/**/*.rs]"
        );
        assert!(pattern.is_match("src/main.rs"));
        assert!(pattern.is_match("crates/foo/src/lib.rs"));
        assert!(!pattern.is_match("tests/main.rs"));
    }

    #[test]
    fn empty_glob_list_matches_nothing() {
        let pattern = serde_saphyr::from_str::<FilePattern>("glob: []").unwrap();
        assert!(!pattern.is_match("any/file.rs"));
        assert!(!pattern.is_match(""));
    }

    #[test]
    fn invalid_glob_pattern_errors() {
        let err = serde_saphyr::from_str::<FilePattern>("glob: \"[\"")
            .expect_err("invalid glob should fail");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("glob"),
            "error should mention glob issues: {msg}"
        );
    }

    #[test]
    fn parse_repos() {
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    entry: cargo fmt --
                    language: system
        "};
        let result = serde_saphyr::from_str::<Config>(yaml).unwrap();
        insta::assert_debug_snapshot!(result);

        // Local hook should not have `rev`
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                rev: v1.0.0
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    language: system
                    entry: cargo fmt
                    types:
                      - rust
        "};
        // Error on extra `rev` field, but not other fields
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 2 column 5: `rev` is not allowed for local repos at line 2, column 5
         --> <input>:2:5
          |
        1 | repos:
        2 |   - repo: local
          |     ^ `rev` is not allowed for local repos at line 2, column 5
        3 |     rev: v1.0.0
        4 |     hooks:
          |
        ");

        // Allow but warn on extra fields (other than `rev`)
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                unknown_field: some_value
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    entry: cargo fmt
                    language: system
                    types:
                      - rust
        "};
        let result = serde_saphyr::from_str::<Config>(yaml).unwrap();
        insta::assert_debug_snapshot!(result);

        // Remote hook should have `rev`.
        let yaml = indoc::indoc! {r"
            repos:
              - repo: https://github.com/crate-ci/typos
                rev: v1.0.0
                hooks:
                  - id: typos
        "};
        let result = serde_saphyr::from_str::<Config>(yaml).unwrap();
        insta::assert_debug_snapshot!(result);

        let yaml = indoc::indoc! {r"
            repos:
              - repo: https://github.com/crate-ci/typos
                hooks:
                  - id: typos
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 2 column 5: missing field `rev` at line 2, column 5
         --> <input>:2:5
          |
        1 | repos:
        2 |   - repo: https://github.com/crate-ci/typos
          |     ^ missing field `rev` at line 2, column 5
        3 |     hooks:
        4 |       - id: typos
          |
        ");

        // Allow `rev` before `repo`
        let yaml = indoc::indoc! {r"
            repos:
              - rev: v1.0.0
                repo: https://github.com/crate-ci/typos
                hooks:
                  - id: typos
        "};
        let result = serde_saphyr::from_str::<Config>(yaml).unwrap();
        insta::assert_debug_snapshot!(result);

        let yaml = indoc::indoc! {r"
            repos:
              - rev: v1.0.0
                repo: local
                hooks:
                  - id: typos
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 5 column 9: missing field `name` at line 5, column 9
         --> <input>:5:9
          |
        3 |     repo: local
        4 |     hooks:
        5 |       - id: typos
          |         ^ missing field `name` at line 5, column 9
        ");

        let yaml = indoc::indoc! {r"
            repos:
              - rev: v1.0.0
                repo: meta
                hooks:
                  - id: typos
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 5 column 9: unknown meta hook id `typos` at line 5, column 9
         --> <input>:5:9
          |
        3 |     repo: meta
        4 |     hooks:
        5 |       - id: typos
          |         ^ unknown meta hook id `typos` at line 5, column 9
        ");

        let yaml = indoc::indoc! {r"
            repos:
              - rev: v1.0.0
                repo: builtin
                hooks:
                  - id: typos
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 5 column 9: unknown builtin hook id `typos` at line 5, column 9
         --> <input>:5:9
          |
        3 |     repo: builtin
        4 |     hooks:
        5 |       - id: typos
          |         ^ unknown builtin hook id `typos` at line 5, column 9
        ");
    }

    #[test]
    fn parse_hooks() {
        // Remote hook only `id` is required.
        let yaml = indoc::indoc! { r"
            repos:
              - repo: https://github.com/crate-ci/typos
                rev: v1.0.0
                hooks:
                  - name: typos
                    alias: typo
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 5 column 9: missing field `id` at line 5, column 9
         --> <input>:5:9
          |
        3 |     rev: v1.0.0
        4 |     hooks:
        5 |       - name: typos
          |         ^ missing field `id` at line 5, column 9
        6 |         alias: typo
          |
        ");

        // Local hook should have `id`, `name`, and `entry` and `language`.
        let yaml = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    entry: cargo fmt
                    types:
                      - rust
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 4 column 9: missing field `language` at line 4, column 9
         --> <input>:4:9
          |
        2 |   - repo: local
        3 |     hooks:
        4 |       - id: cargo-fmt
          |         ^ missing field `language` at line 4, column 9
        5 |         name: cargo fmt
        6 |         entry: cargo fmt
          |
        ");

        let yaml = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    entry: cargo fmt
                    language: rust
        "};
        let result = serde_saphyr::from_str::<Config>(yaml).unwrap();
        insta::assert_debug_snapshot!(result);
    }

    #[test]
    fn meta_hooks() {
        // Invalid rev
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                rev: v1.0.0
                hooks:
                  - name: typos
                    alias: typo
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 5 column 9: missing field `id` at line 5, column 9
         --> <input>:5:9
          |
        3 |     rev: v1.0.0
        4 |     hooks:
        5 |       - name: typos
          |         ^ missing field `id` at line 5, column 9
        6 |         alias: typo
          |
        ");

        // Invalid meta hook id
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                hooks:
                  - id: hello
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 4 column 9: unknown meta hook id `hello` at line 4, column 9
         --> <input>:4:9
          |
        2 |   - repo: meta
        3 |     hooks:
        4 |       - id: hello
          |         ^ unknown meta hook id `hello` at line 4, column 9
        ");

        // Invalid language
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                hooks:
                  - id: check-hooks-apply
                    language: python
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 4 column 9: language must be `system` for meta hooks at line 4, column 9
         --> <input>:4:9
          |
        2 |   - repo: meta
        3 |     hooks:
        4 |       - id: check-hooks-apply
          |         ^ language must be `system` for meta hooks at line 4, column 9
        5 |         language: python
          |
        ");

        // Invalid entry
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                hooks:
                  - id: check-hooks-apply
                    entry: echo hell world
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 4 column 9: `entry` is not allowed for meta hooks at line 4, column 9
         --> <input>:4:9
          |
        2 |   - repo: meta
        3 |     hooks:
        4 |       - id: check-hooks-apply
          |         ^ `entry` is not allowed for meta hooks at line 4, column 9
        5 |         entry: echo hell world
          |
        ");

        // Valid meta hook
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                hooks:
                  - id: check-hooks-apply
                  - id: check-useless-excludes
                  - id: identity
        "};
        let result = serde_saphyr::from_str::<Config>(yaml).unwrap();
        insta::assert_debug_snapshot!(result);
    }

    #[test]
    fn language_version() {
        let yaml = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: hook-1
                    name: hook 1
                    entry: echo hello world
                    language: system
                    language_version: default
                  - id: hook-2
                    name: hook 2
                    entry: echo hello world
                    language: system
                    language_version: system
                  - id: hook-3
                    name: hook 3
                    entry: echo hello world
                    language: system
                    language_version: '3.8'
        "};
        let result = serde_saphyr::from_str::<Config>(yaml);
        insta::assert_debug_snapshot!(result);
    }

    #[test]
    fn test_read_yaml_config() -> Result<()> {
        let config = read_config(Path::new("tests/fixtures/uv-pre-commit-config.yaml"))?;
        insta::assert_debug_snapshot!(config);
        Ok(())
    }

    #[test]
    fn test_read_toml_config() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let toml_path = dir.path().join("prek.toml");
        fs_err::write(
            &toml_path,
            indoc::indoc! {r#"
            fail_fast = true

            [[repos]]
            repo = "local"

            [[repos.hooks]]
            id = "cargo-fmt"
            name = "cargo fmt"
            entry = "cargo fmt --"
            language = "system"

            [[repos]]
            repo = "https://github.com/pre-commit/pre-commit-hooks"
            rev = "v6.0.0"
            hooks = [
            { id = "trailing-whitespace" },
            {
                id = "end-of-file-fixer",
                args = ["--fix", "crlf"]
            }
            ]
        "#},
        )?;

        let config = read_config(&toml_path)?;
        insta::assert_debug_snapshot!(config);

        Ok(())
    }

    #[test]
    fn test_read_invalid_toml_config() {
        let raw = indoc::indoc! {r#"
            fail_fast = true

            [[repos]]
            repo = "local"

            [[repos.hooks]]
            id = "cargo-fmt"
            name = "cargo fmt"
            entry = "cargo fmt --"
            language = "system"

            [[repos]]
            repo = "https://github.com/pre-commit/pre-commit-hooks"
            hooks = [
            { id = "trailing-whitespace" },
            {
                id = "end-of-file-fixer",
                args = ["--fix", "crlf"]
            }
            ]
        "#};

        let err = toml::from_str::<Config>(raw).unwrap_err();
        insta::assert_snapshot!(err, @"
        TOML parse error at line 12, column 1
           |
        12 | [[repos]]
           | ^^^^^^^^^
        missing field `rev`
        ");

        let raw = indoc::indoc! {r#"
            fail_fast = true

            [[repos]]
            repo = "local"
            rev = "v1.0.0"

            [[repos.hooks]]
            id = "cargo-fmt"
            name = "cargo fmt"
            entry = "cargo fmt --"
            language = "system"

            [[repos]]
            repo = "https://github.com/pre-commit/pre-commit-hooks"
            rev = "v6.0.0"
            hooks = [
            { id = "trailing-whitespace" },
            {
                id = "end-of-file-fixer",
                args = ["--fix", "crlf"]
            }
            ]
        "#};

        let err = toml::from_str::<Config>(raw).unwrap_err();
        insta::assert_snapshot!(err, @"
        TOML parse error at line 3, column 1
          |
        3 | [[repos]]
          | ^^^^^^^^^
        `rev` is not allowed for local repos
        ");
    }

    #[test]
    fn test_read_manifest() -> Result<()> {
        let manifest = read_manifest(Path::new("tests/fixtures/uv-pre-commit-hooks.yaml"))?;
        insta::assert_debug_snapshot!(manifest);
        Ok(())
    }

    #[test]
    fn test_minimum_prek_version() {
        // Test that missing minimum_prek_version field doesn't cause an error
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: test-hook
                    name: Test Hook
                    entry: echo test
                    language: system
        "};
        let config = serde_saphyr::from_str::<Config>(yaml).unwrap();
        assert!(config.minimum_prek_version.is_none());

        // Test that empty minimum_prek_version field is treated as None
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: test-hook
                    name: Test Hook
                    entry: echo test
                    language: system
            minimum_prek_version: ''
        "};
        let config = serde_saphyr::from_str::<Config>(yaml).unwrap();
        assert!(config.minimum_prek_version.is_none());

        // Test that valid minimum_prek_version field works in top-level config
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: test-hook
                    name: Test Hook
                    entry: echo test
                    language: system
            minimum_prek_version: '10.0.0'
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::with_settings!({ filters => vec![VERSION_FILTER] }, {
            insta::assert_snapshot!(err, @"
            error: line 8 column 23: Required minimum prek version `10.0.0` is greater than current version `[CURRENT_VERSION]`; Please consider updating prek at line 8, column 23
             --> <input>:8:23
              |
            6 |         entry: echo test
            7 |         language: system
            8 | minimum_prek_version: '10.0.0'
              |                       ^ Required minimum prek version `10.0.0` is greater than current version `[CURRENT_VERSION]`; Please consider updating prek at line 8, column 23
            ");
        });

        // Test that valid minimum_prek_version field works in hook config
        let yaml = indoc::indoc! {r"
          - id: test-hook
            name: Test Hook
            entry: echo test
            language: system
            minimum_prek_version: '10.0.0'
        "};
        let err = serde_saphyr::from_str::<Manifest>(yaml).unwrap_err();
        insta::with_settings!({ filters => vec![VERSION_FILTER] }, {
            insta::assert_snapshot!(err, @"
            error: line 1 column 3: Required minimum prek version `10.0.0` is greater than current version `[CURRENT_VERSION]`; Please consider updating prek at line 1, column 3
             --> <input>:1:3
              |
            1 | - id: test-hook
              |   ^ Required minimum prek version `10.0.0` is greater than current version `[CURRENT_VERSION]`; Please consider updating prek at line 1, column 3
            2 |   name: Test Hook
            3 |   entry: echo test
              |
            ");
        });
    }

    #[test]
    fn test_validate_type_tags() {
        // Valid tags should parse successfully
        let yaml_valid = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: my-hook
                    name: My Hook
                    entry: echo
                    language: system
                    types: [python, file]
                    types_or: [text, binary]
                    exclude_types: [symlink]
        "};
        let result = serde_saphyr::from_str::<Config>(yaml_valid);
        assert!(result.is_ok(), "Should parse valid tags successfully");

        // Empty lists and missing keys should also be fine
        let yaml_empty = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: my-hook
                    name: My Hook
                    entry: echo
                    language: system
                    types: []
                    exclude_types: []
                    # types_or is missing, which is also valid
        "};
        let result_empty = serde_saphyr::from_str::<Config>(yaml_empty);
        assert!(
            result_empty.is_ok(),
            "Should parse empty/missing tags successfully"
        );

        // Invalid tag in 'types' should fail
        let yaml_invalid_types = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: my-hook
                    name: My Hook
                    entry: echo
                    language: system
                    types: [pythoon] # Deliberate typo
        "};
        let err = serde_saphyr::from_str::<Config>(yaml_invalid_types).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 4 column 9: Type tag `pythoon` is not recognized. Check for typos or upgrade prek to get new tags. at line 4, column 9
         --> <input>:4:9
          |
        2 |   - repo: local
        3 |     hooks:
        4 |       - id: my-hook
          |         ^ Type tag `pythoon` is not recognized. Check for typos or upgrade prek to get new tags. at line 4, column 9
        5 |         name: My Hook
        6 |         entry: echo
          |
        ");

        // Invalid tag in 'types_or' should fail
        let yaml_invalid_types_or = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: my-hook
                    name: My Hook
                    entry: echo
                    language: system
                    types_or: [invalidtag]
        "};
        let err = serde_saphyr::from_str::<Config>(yaml_invalid_types_or).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 4 column 9: Type tag `invalidtag` is not recognized. Check for typos or upgrade prek to get new tags. at line 4, column 9
         --> <input>:4:9
          |
        2 |   - repo: local
        3 |     hooks:
        4 |       - id: my-hook
          |         ^ Type tag `invalidtag` is not recognized. Check for typos or upgrade prek to get new tags. at line 4, column 9
        5 |         name: My Hook
        6 |         entry: echo
          |
        ");

        // Invalid tag in 'exclude_types' should fail
        let yaml_invalid_exclude_types = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: my-hook
                    name: My Hook
                    entry: echo
                    language: system
                    exclude_types: [not-a-real-tag]
        "};
        let err = serde_saphyr::from_str::<Config>(yaml_invalid_exclude_types).unwrap_err();
        insta::assert_snapshot!(err, @"
        error: line 4 column 9: Type tag `not-a-real-tag` is not recognized. Check for typos or upgrade prek to get new tags. at line 4, column 9
         --> <input>:4:9
          |
        2 |   - repo: local
        3 |     hooks:
        4 |       - id: my-hook
          |         ^ Type tag `not-a-real-tag` is not recognized. Check for typos or upgrade prek to get new tags. at line 4, column 9
        5 |         name: My Hook
        6 |         entry: echo
          |
        ");
    }

    #[test]
    fn read_config_with_merge_keys() -> Result<()> {
        let yaml = indoc::indoc! {r#"
            repos:
              - repo: local
                hooks:
                  - id: mypy-local
                    name: Local mypy
                    entry: python tools/pre_commit/mypy.py 0 "local"
                    <<: &mypy_common
                      language: python
                      types_or: [python, pyi]
                  - id: mypy-3.10
                    name: Mypy 3.10
                    entry: python tools/pre_commit/mypy.py 1 "3.10"
                    <<: *mypy_common
        "#};

        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(yaml.as_bytes())?;

        let config = read_config(file.path())?;
        insta::assert_debug_snapshot!(config);

        Ok(())
    }

    #[test]
    fn read_config_with_nested_merge_keys() -> Result<()> {
        let yaml = indoc::indoc! {r"
            local: &local
              language: system
              pass_filenames: false
              require_serial: true

            local-commit: &local-commit
              <<: *local
              stages: [pre-commit]

            repos:
            - repo: local
              hooks:
              - id: test-yaml
                name: Test YAML compatibility
                entry: prek --help
                <<: *local-commit
        "};

        let mut file = tempfile::NamedTempFile::new()?;
        file.write_all(yaml.as_bytes())?;

        let config = read_config(file.path())?;
        insta::assert_debug_snapshot!(config);

        Ok(())
    }

    #[test]
    fn test_list_with_unindented_square() {
        let yaml = indoc::indoc! {r#"
        repos:
          - repo: https://github.com/pre-commit/mirrors-mypy
            rev: v1.18.2
            hooks:
              - id: mypy
                exclude: tests/data
                args: [ "--pretty", "--show-error-codes" ]
                additional_dependencies: [
                  'keyring==24.2.0',
                  'nox==2024.03.02',
                  'pytest',
                  'types-docutils==0.20.0.3',
                  'types-setuptools==68.2.0.0',
                  'types-freezegun==1.1.10',
                  'types-pyyaml==6.0.12.12',
                  'typing-extensions',
                ]
        "#};
        let result = serde_saphyr::from_str::<Config>(yaml);
        assert!(result.is_ok());
    }

    #[test]
    fn test_numeric_rev_is_parsed_as_string() {
        // Because we define `rev` as a String, `serde-saphyr` can automatically parse numeric
        // revs as strings.
        let yaml = indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/mirrors-mypy
            rev: 1.0
            hooks:
              - id: mypy
        "};
        let config = serde_saphyr::from_str::<Config>(yaml).unwrap();
        insta::assert_debug_snapshot!(config);
    }
}
