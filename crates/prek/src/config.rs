#[cfg(feature = "schemars")]
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::ops::RangeInclusive;
use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use fancy_regex::Regex;
use globset::{Glob, GlobSet, GlobSetBuilder};
use itertools::Itertools;
use prek_consts::{ALT_CONFIG_FILE, CONFIG_FILE};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer, Serialize};
use tracing::instrument;

use crate::fs::Simplified;
use crate::version;
use crate::warn_user;
use crate::{identify, yaml};

pub(crate) static CONFIG_FILE_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = format!(
        "^{}|{}$",
        fancy_regex::escape(CONFIG_FILE),
        fancy_regex::escape(ALT_CONFIG_FILE)
    );
    Regex::new(&pattern).expect("config regex must compile")
});

#[derive(Debug, Clone)]
pub(crate) struct GlobPatterns {
    patterns: Vec<String>,
    set: GlobSet,
}

impl GlobPatterns {
    fn new(patterns: Vec<String>) -> Result<Self, globset::Error> {
        let mut builder = GlobSetBuilder::new();
        for pattern in &patterns {
            builder.add(Glob::new(pattern)?);
        }
        let set = builder.build()?;
        Ok(Self { patterns, set })
    }

    fn is_match(&self, value: &str) -> bool {
        self.set.is_match(Path::new(value))
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

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for FilePattern {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("FilePattern")
    }

    fn json_schema(_gen: &mut schemars::generate::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "description": "A file pattern, either a regex or glob pattern(s).",
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "regex": {
                            "type": "string",
                            "description": "A regular expression pattern.",
                        }
                    },
                    "required": ["regex"],
                },
                {
                    "type": "object",
                    "properties": {
                        "glob": {
                            "oneOf": [
                                {
                                    "type": "string",
                                    "description": "A glob pattern.",
                                },
                                {
                                    "type": "array",
                                    "items": {
                                        "type": "string",
                                    },
                                    "description": "A list of glob patterns.",
                                }
                            ]
                        }
                    },
                    "required": ["glob"],
                }
            ],
        })
    }
}

impl FilePattern {
    pub(crate) fn is_match(&self, str: &str) -> bool {
        match self {
            FilePattern::Regex(regex) => regex.is_match(str).unwrap_or(false),
            FilePattern::Glob(globs) => globs.is_match(str),
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

impl From<Regex> for FilePattern {
    fn from(regex: Regex) -> Self {
        FilePattern::Regex(regex)
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

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize, Serialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum Language {
    Conda,
    Coursier,
    Dart,
    Docker,
    DockerImage,
    Dotnet,
    Fail,
    Golang,
    Haskell,
    Lua,
    Node,
    Perl,
    Python,
    R,
    Ruby,
    Rust,
    Swift,
    Pygrep,
    #[serde(alias = "unsupported_script")]
    Script,
    #[serde(alias = "unsupported")]
    System,
}

impl Language {
    pub fn as_str(&self) -> &str {
        match self {
            Self::Conda => "conda",
            Self::Coursier => "coursier",
            Self::Dart => "dart",
            Self::Docker => "docker",
            Self::DockerImage => "docker_image",
            Self::Dotnet => "dotnet",
            Self::Fail => "fail",
            Self::Golang => "golang",
            Self::Haskell => "haskell",
            Self::Lua => "lua",
            Self::Node => "node",
            Self::Perl => "perl",
            Self::Python => "python",
            Self::R => "r",
            Self::Ruby => "ruby",
            Self::Rust => "rust",
            Self::Swift => "swift",
            Self::Pygrep => "pygrep",
            Self::Script => "script",
            Self::System => "system",
        }
    }
}

impl Display for Language {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
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
    pub fn as_str(&self) -> &str {
        match self {
            Self::CommitMsg => "commit-msg",
            Self::PostCheckout => "post-checkout",
            Self::PostCommit => "post-commit",
            Self::PostMerge => "post-merge",
            Self::PostRewrite => "post-rewrite",
            Self::PreCommit => "pre-commit",
            Self::PreMergeCommit => "pre-merge-commit",
            Self::PrePush => "pre-push",
            Self::PreRebase => "pre-rebase",
            Self::PrepareCommitMsg => "prepare-commit-msg",
        }
    }

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

impl Display for HookType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, Hash, Deserialize, Serialize, clap::ValueEnum,
)]
#[serde(rename_all = "kebab-case")]
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
    pub fn as_str(&self) -> &str {
        match self {
            Self::Manual => "manual",
            Self::CommitMsg => "commit-msg",
            Self::PostCheckout => "post-checkout",
            Self::PostCommit => "post-commit",
            Self::PostMerge => "post-merge",
            Self::PostRewrite => "post-rewrite",
            Self::PreCommit => "pre-commit",
            Self::PreMergeCommit => "pre-merge-commit",
            Self::PrePush => "pre-push",
            Self::PreRebase => "pre-rebase",
            Self::PrepareCommitMsg => "prepare-commit-msg",
        }
    }
}

impl Display for Stage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
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
    /// Priority used by the scheduler to determine ordering and concurrency.
    /// Hooks with the same priority can run in parallel.
    pub priority: Option<u32>,
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
    #[serde(skip_serializing)]
    #[serde(flatten)]
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
            priority,
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
    #[serde(flatten)]
    pub options: HookOptions,
}

/// A local hook in the configuration file.
///
/// It's the same as the manifest hook definition.
pub(crate) type LocalHook = ManifestHook;

/// A meta hook predefined in pre-commit.
///
/// It's the same as the manifest hook definition but with only a few predefined id allowed.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(try_from = "RemoteHook")]
pub(crate) struct MetaHook(pub(crate) ManifestHook);

#[derive(Debug, thiserror::Error)]
pub(crate) enum MetaHookWireError {
    #[error("unknown meta hook id `{0}`")]
    UnknownId(String),

    #[error("language must be `system` for meta hooks")]
    InvalidLanguage,

    #[error("entry is not allowed for meta hooks")]
    EntryNotAllowed,
}

impl TryFrom<RemoteHook> for MetaHook {
    type Error = MetaHookWireError;

    fn try_from(hook_options: RemoteHook) -> std::result::Result<Self, Self::Error> {
        let mut meta_hook = MetaHook::from_id(&hook_options.id)
            .map_err(|()| MetaHookWireError::UnknownId(hook_options.id.clone()))?;

        if hook_options.language.is_some_and(|l| l != Language::System) {
            return Err(MetaHookWireError::InvalidLanguage);
        }
        if hook_options.entry.is_some() {
            return Err(MetaHookWireError::EntryNotAllowed);
        }

        if let Some(name) = &hook_options.name {
            meta_hook.0.name.clone_from(name);
        }
        meta_hook.0.options.update(&hook_options.options);

        Ok(meta_hook)
    }
}

impl From<MetaHook> for ManifestHook {
    fn from(hook: MetaHook) -> Self {
        hook.0
    }
}

/// A builtin hook predefined in prek.
/// Basically the same as meta hooks, but defined under `builtin` repo, and do other non-meta checks.
#[derive(Debug, Clone, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(try_from = "RemoteHook")]
pub(crate) struct BuiltinHook(pub(crate) ManifestHook);

#[derive(Debug, thiserror::Error)]
pub(crate) enum BuiltinHookWireError {
    #[error("unknown builtin hook id `{0}`")]
    UnknownId(String),

    #[error("language must be `system` for builtin hooks")]
    InvalidLanguage,

    #[error("entry is not allowed for builtin hooks")]
    EntryNotAllowed,
}

impl TryFrom<RemoteHook> for BuiltinHook {
    type Error = BuiltinHookWireError;

    fn try_from(hook_options: RemoteHook) -> std::result::Result<Self, Self::Error> {
        let mut builtin_hook = BuiltinHook::from_id(&hook_options.id)
            .map_err(|()| BuiltinHookWireError::UnknownId(hook_options.id.clone()))?;

        if hook_options.language.is_some_and(|l| l != Language::System) {
            return Err(BuiltinHookWireError::InvalidLanguage);
        }
        if hook_options.entry.is_some() {
            return Err(BuiltinHookWireError::EntryNotAllowed);
        }

        if let Some(name) = &hook_options.name {
            builtin_hook.0.name.clone_from(name);
        }
        builtin_hook.0.options.update(&hook_options.options);

        Ok(builtin_hook)
    }
}

impl From<BuiltinHook> for ManifestHook {
    fn from(hook: BuiltinHook) -> Self {
        hook.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) struct RemoteRepo {
    pub repo: String,
    pub rev: String,
    #[serde(skip_serializing)]
    pub hooks: Vec<RemoteHook>,
    #[serde(skip_serializing)]
    #[serde(flatten)]
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

// TODO: resolve if `repo` is a local relative path before comparing
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
    pub repo: String,
    pub hooks: Vec<LocalHook>,
    #[serde(skip_serializing)]
    #[serde(flatten)]
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
    pub repo: String,
    pub hooks: Vec<MetaHook>,
    #[serde(skip_serializing)]
    #[serde(flatten)]
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
    pub repo: String,
    pub hooks: Vec<BuiltinHook>,
    #[serde(skip_serializing)]
    #[serde(flatten)]
    _unused_keys: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "serde_json::Value")]
pub(crate) enum Repo {
    Remote(RemoteRepo),
    Local(LocalRepo),
    Meta(MetaRepo),
    Builtin(BuiltinRepo),
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RepoWireError {
    #[error("missing field `repo`")]
    MissingRepo,

    #[error("repo must be a string")]
    RepoNotString,

    #[error("Invalid local repo: {0}")]
    InvalidLocal(String),

    #[error("Invalid meta repo: {0}")]
    InvalidMeta(String),

    #[error("Invalid builtin repo: {0}")]
    InvalidBuiltin(String),

    #[error("Invalid remote repo: {0}")]
    InvalidRemote(String),
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for Repo {
    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("Repo")
    }

    fn json_schema(r#gen: &mut schemars::generate::SchemaGenerator) -> schemars::Schema {
        let remote_schema = r#gen.subschema_for::<RemoteRepo>();
        let local_schema = r#gen.subschema_for::<LocalRepo>();
        let meta_schema = r#gen.subschema_for::<MetaRepo>();
        let builtin_schema = r#gen.subschema_for::<BuiltinRepo>();

        schemars::json_schema!({
            "type": "object",
            "description": "A repository of hooks, which can be remote, local, meta, or builtin.",
            "oneOf": [
                remote_schema,
                local_schema,
                meta_schema,
                builtin_schema,
            ],
        })
    }
}

impl TryFrom<serde_json::Value> for Repo {
    type Error = RepoWireError;

    fn try_from(repo_wire: serde_json::Value) -> std::result::Result<Self, Self::Error> {
        let repo_location = repo_wire
            .get("repo")
            .ok_or(RepoWireError::MissingRepo)?
            .as_str()
            .ok_or(RepoWireError::RepoNotString)?
            .to_string();

        match repo_location.as_str() {
            "local" => LocalRepo::deserialize(repo_wire)
                .map(Repo::Local)
                .map_err(|e| RepoWireError::InvalidLocal(e.to_string())),
            "meta" => MetaRepo::deserialize(repo_wire)
                .map(Repo::Meta)
                .map_err(|e| RepoWireError::InvalidMeta(e.to_string())),
            "builtin" => BuiltinRepo::deserialize(repo_wire)
                .map(Repo::Builtin)
                .map_err(|e| RepoWireError::InvalidBuiltin(e.to_string())),
            _ => RemoteRepo::deserialize(repo_wire)
                .map(Repo::Remote)
                .map_err(|e| RepoWireError::InvalidRemote(e.to_string())),
        }
    }
}

// TODO: warn sensible regex
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
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

    #[serde(skip_serializing)]
    #[serde(flatten)]
    _unused_keys: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error("Config file not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("Failed to parse `{0}`")]
    Yaml(String, #[source] serde_yaml::Error),

    #[error("Failed to merge keys in `{0}`")]
    YamlMerge(String, #[source] yaml::MergeKeyError),
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
                    Box::new(meta.hooks.iter().map(|h| &h.0.options)),
                ),
                Repo::Builtin(builtin) => (
                    &builtin._unused_keys,
                    Box::new(builtin.hooks.iter().map(|h| &h.0.options)),
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
    let content = match fs_err::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::NotFound(path.user_display().to_string()));
        }
        Err(e) => return Err(e.into()),
    };

    let config: serde_yaml::Value = serde_yaml::from_str(&content)
        .map_err(|e| Error::Yaml(path.user_display().to_string(), e))?;

    let config = yaml::merge_keys(config)
        .map_err(|e| Error::YamlMerge(path.user_display().to_string(), e))?;

    let config: Config = serde_yaml::from_value(config)
        .map_err(|e| Error::Yaml(path.user_display().to_string(), e))?;

    Ok(config)
}

/// Read the configuration file from the given path, and warn about certain issues.
#[instrument(level = "trace")]
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
            Hint: `prek auto-update` often fixes this",
            "#,
            msg
            }
        );
    }

    Ok(config)
}

// TODO: disallow `priority` in manifest

/// Read the manifest file from the given path.
pub(crate) fn read_manifest(path: &Path) -> Result<Manifest, Error> {
    let content = fs_err::read_to_string(path)?;
    let manifest = serde_yaml::from_str(&content)
        .map_err(|e| Error::Yaml(path.user_display().to_string(), e))?;
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
        return Err(serde::de::Error::custom(format!(
            "Required minimum prek version `{version}` is greater than current version `{cur_version}`. Please consider updating prek.",
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
                let msg = format!("Type tag \"{tag}\" is not recognized. Try upgrading prek");
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
            serde_yaml::from_str(regex_yaml).expect("regex patterns should parse");
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
        let parsed: Wrapper = serde_yaml::from_str(glob_yaml).expect("glob patterns should parse");
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
            serde_yaml::from_str(glob_list_yaml).expect("glob list patterns should parse");
        assert!(parsed.files.is_match("src/lib/main.rs"));
        assert!(parsed.files.is_match("crates/foo/src/lib.rs"));
        assert!(!parsed.files.is_match("tests/main.rs"));
        assert!(parsed.exclude.is_match("target/debug/app"));
        assert!(parsed.exclude.is_match("dist/app"));
    }

    #[test]
    fn file_patterns_expose_sources_and_display() {
        let pattern: FilePattern = serde_yaml::from_str(indoc::indoc! {r"
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
        let pattern = serde_yaml::from_str::<FilePattern>("glob: []").unwrap();
        assert!(!pattern.is_match("any/file.rs"));
        assert!(!pattern.is_match(""));
    }

    #[test]
    fn invalid_glob_pattern_errors() {
        let err = serde_yaml::from_str::<FilePattern>("glob: \"[\"")
            .expect_err("invalid glob should fail");
        let msg = err.to_string().to_lowercase();
        assert!(
            msg.contains("glob"),
            "error should mention glob issues: {msg}"
        );
    }

    #[test]
    fn parse_repos() {
        // Local hook should not have `rev`
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    entry: cargo fmt --
                    language: system
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_debug_snapshot!(result, @r#"
        Ok(
            Config {
                repos: [
                    Local(
                        LocalRepo {
                            repo: "local",
                            hooks: [
                                ManifestHook {
                                    id: "cargo-fmt",
                                    name: "cargo fmt",
                                    entry: "cargo fmt --",
                                    language: System,
                                    options: HookOptions {
                                        alias: None,
                                        files: None,
                                        exclude: None,
                                        types: None,
                                        types_or: None,
                                        exclude_types: None,
                                        additional_dependencies: None,
                                        args: None,
                                        env: None,
                                        always_run: None,
                                        fail_fast: None,
                                        pass_filenames: None,
                                        description: None,
                                        language_version: None,
                                        log_file: None,
                                        require_serial: None,
                                        priority: None,
                                        stages: None,
                                        verbose: None,
                                        minimum_prek_version: None,
                                        _unused_keys: {},
                                    },
                                },
                            ],
                            _unused_keys: {},
                        },
                    ),
                ],
                default_install_hook_types: None,
                default_language_version: None,
                default_stages: None,
                files: None,
                exclude: None,
                fail_fast: None,
                minimum_prek_version: None,
                orphan: None,
                _unused_keys: {},
            },
        )
        "#);

        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                rev: v1.0.0
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    types:
                      - rust
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_snapshot!(result.unwrap_err().to_string(), @"repos: Invalid local repo: missing field `entry` at line 2 column 3");

        // Remote hook should have `rev`.
        let yaml = indoc::indoc! {r"
            repos:
              - repo: https://github.com/crate-ci/typos
                rev: v1.0.0
                hooks:
                  - id: typos
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_debug_snapshot!(result, @r#"
        Ok(
            Config {
                repos: [
                    Remote(
                        RemoteRepo {
                            repo: "https://github.com/crate-ci/typos",
                            rev: "v1.0.0",
                            hooks: [
                                RemoteHook {
                                    id: "typos",
                                    name: None,
                                    entry: None,
                                    language: None,
                                    options: HookOptions {
                                        alias: None,
                                        files: None,
                                        exclude: None,
                                        types: None,
                                        types_or: None,
                                        exclude_types: None,
                                        additional_dependencies: None,
                                        args: None,
                                        env: None,
                                        always_run: None,
                                        fail_fast: None,
                                        pass_filenames: None,
                                        description: None,
                                        language_version: None,
                                        log_file: None,
                                        require_serial: None,
                                        priority: None,
                                        stages: None,
                                        verbose: None,
                                        minimum_prek_version: None,
                                        _unused_keys: {},
                                    },
                                },
                            ],
                            _unused_keys: {},
                        },
                    ),
                ],
                default_install_hook_types: None,
                default_language_version: None,
                default_stages: None,
                files: None,
                exclude: None,
                fail_fast: None,
                minimum_prek_version: None,
                orphan: None,
                _unused_keys: {},
            },
        )
        "#);

        let yaml = indoc::indoc! {r"
            repos:
              - repo: https://github.com/crate-ci/typos
                hooks:
                  - id: typos
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_snapshot!(result.unwrap_err().to_string(), @"repos: Invalid remote repo: missing field `rev` at line 2 column 3");
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
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_snapshot!(result.unwrap_err().to_string(), @"repos: Invalid remote repo: missing field `id` at line 2 column 3");

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
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_snapshot!(result.unwrap_err().to_string(), @"repos: Invalid local repo: missing field `language` at line 2 column 3");

        let yaml = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    entry: cargo fmt
                    language: rust
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_debug_snapshot!(result, @r#"
        Ok(
            Config {
                repos: [
                    Local(
                        LocalRepo {
                            repo: "local",
                            hooks: [
                                ManifestHook {
                                    id: "cargo-fmt",
                                    name: "cargo fmt",
                                    entry: "cargo fmt",
                                    language: Rust,
                                    options: HookOptions {
                                        alias: None,
                                        files: None,
                                        exclude: None,
                                        types: None,
                                        types_or: None,
                                        exclude_types: None,
                                        additional_dependencies: None,
                                        args: None,
                                        env: None,
                                        always_run: None,
                                        fail_fast: None,
                                        pass_filenames: None,
                                        description: None,
                                        language_version: None,
                                        log_file: None,
                                        require_serial: None,
                                        priority: None,
                                        stages: None,
                                        verbose: None,
                                        minimum_prek_version: None,
                                        _unused_keys: {},
                                    },
                                },
                            ],
                            _unused_keys: {},
                        },
                    ),
                ],
                default_install_hook_types: None,
                default_language_version: None,
                default_stages: None,
                files: None,
                exclude: None,
                fail_fast: None,
                minimum_prek_version: None,
                orphan: None,
                _unused_keys: {},
            },
        )
        "#);
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
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_snapshot!(result.unwrap_err().to_string(), @"repos: Invalid meta repo: missing field `id` at line 2 column 3");

        // Invalid meta hook id
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                hooks:
                  - id: hello
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_snapshot!(result.unwrap_err().to_string(), @"repos: Invalid meta repo: unknown meta hook id `hello` at line 2 column 3");

        // Invalid language
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                hooks:
                  - id: check-hooks-apply
                    language: python
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_snapshot!(result.unwrap_err().to_string(), @"repos: Invalid meta repo: language must be `system` for meta hooks at line 2 column 3");

        // Invalid entry
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                hooks:
                  - id: check-hooks-apply
                    entry: echo hell world
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_snapshot!(result.unwrap_err().to_string(), @"repos: Invalid meta repo: entry is not allowed for meta hooks at line 2 column 3");

        // Valid meta hook
        let yaml = indoc::indoc! { r"
            repos:
              - repo: meta
                hooks:
                  - id: check-hooks-apply
                  - id: check-useless-excludes
                  - id: identity
        "};
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_debug_snapshot!(result, @r#"
        Ok(
            Config {
                repos: [
                    Meta(
                        MetaRepo {
                            repo: "meta",
                            hooks: [
                                MetaHook(
                                    ManifestHook {
                                        id: "check-hooks-apply",
                                        name: "Check hooks apply",
                                        entry: "",
                                        language: System,
                                        options: HookOptions {
                                            alias: None,
                                            files: Some(
                                                Regex(
                                                    ^\.pre-commit-config\.yaml|\.pre-commit-config\.yml$,
                                                ),
                                            ),
                                            exclude: None,
                                            types: None,
                                            types_or: None,
                                            exclude_types: None,
                                            additional_dependencies: None,
                                            args: None,
                                            env: None,
                                            always_run: None,
                                            fail_fast: None,
                                            pass_filenames: None,
                                            description: None,
                                            language_version: None,
                                            log_file: None,
                                            require_serial: None,
                                            priority: None,
                                            stages: None,
                                            verbose: None,
                                            minimum_prek_version: None,
                                            _unused_keys: {},
                                        },
                                    },
                                ),
                                MetaHook(
                                    ManifestHook {
                                        id: "check-useless-excludes",
                                        name: "Check useless excludes",
                                        entry: "",
                                        language: System,
                                        options: HookOptions {
                                            alias: None,
                                            files: Some(
                                                Regex(
                                                    ^\.pre-commit-config\.yaml|\.pre-commit-config\.yml$,
                                                ),
                                            ),
                                            exclude: None,
                                            types: None,
                                            types_or: None,
                                            exclude_types: None,
                                            additional_dependencies: None,
                                            args: None,
                                            env: None,
                                            always_run: None,
                                            fail_fast: None,
                                            pass_filenames: None,
                                            description: None,
                                            language_version: None,
                                            log_file: None,
                                            require_serial: None,
                                            priority: None,
                                            stages: None,
                                            verbose: None,
                                            minimum_prek_version: None,
                                            _unused_keys: {},
                                        },
                                    },
                                ),
                                MetaHook(
                                    ManifestHook {
                                        id: "identity",
                                        name: "identity",
                                        entry: "",
                                        language: System,
                                        options: HookOptions {
                                            alias: None,
                                            files: None,
                                            exclude: None,
                                            types: None,
                                            types_or: None,
                                            exclude_types: None,
                                            additional_dependencies: None,
                                            args: None,
                                            env: None,
                                            always_run: None,
                                            fail_fast: None,
                                            pass_filenames: None,
                                            description: None,
                                            language_version: None,
                                            log_file: None,
                                            require_serial: None,
                                            priority: None,
                                            stages: None,
                                            verbose: Some(
                                                true,
                                            ),
                                            minimum_prek_version: None,
                                            _unused_keys: {},
                                        },
                                    },
                                ),
                            ],
                            _unused_keys: {},
                        },
                    ),
                ],
                default_install_hook_types: None,
                default_language_version: None,
                default_stages: None,
                files: None,
                exclude: None,
                fail_fast: None,
                minimum_prek_version: None,
                orphan: None,
                _unused_keys: {},
            },
        )
        "#);
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
        let result = serde_yaml::from_str::<Config>(yaml);
        insta::assert_debug_snapshot!(result, @r#"
        Ok(
            Config {
                repos: [
                    Local(
                        LocalRepo {
                            repo: "local",
                            hooks: [
                                ManifestHook {
                                    id: "hook-1",
                                    name: "hook 1",
                                    entry: "echo hello world",
                                    language: System,
                                    options: HookOptions {
                                        alias: None,
                                        files: None,
                                        exclude: None,
                                        types: None,
                                        types_or: None,
                                        exclude_types: None,
                                        additional_dependencies: None,
                                        args: None,
                                        env: None,
                                        always_run: None,
                                        fail_fast: None,
                                        pass_filenames: None,
                                        description: None,
                                        language_version: Some(
                                            "default",
                                        ),
                                        log_file: None,
                                        require_serial: None,
                                        priority: None,
                                        stages: None,
                                        verbose: None,
                                        minimum_prek_version: None,
                                        _unused_keys: {},
                                    },
                                },
                                ManifestHook {
                                    id: "hook-2",
                                    name: "hook 2",
                                    entry: "echo hello world",
                                    language: System,
                                    options: HookOptions {
                                        alias: None,
                                        files: None,
                                        exclude: None,
                                        types: None,
                                        types_or: None,
                                        exclude_types: None,
                                        additional_dependencies: None,
                                        args: None,
                                        env: None,
                                        always_run: None,
                                        fail_fast: None,
                                        pass_filenames: None,
                                        description: None,
                                        language_version: Some(
                                            "system",
                                        ),
                                        log_file: None,
                                        require_serial: None,
                                        priority: None,
                                        stages: None,
                                        verbose: None,
                                        minimum_prek_version: None,
                                        _unused_keys: {},
                                    },
                                },
                                ManifestHook {
                                    id: "hook-3",
                                    name: "hook 3",
                                    entry: "echo hello world",
                                    language: System,
                                    options: HookOptions {
                                        alias: None,
                                        files: None,
                                        exclude: None,
                                        types: None,
                                        types_or: None,
                                        exclude_types: None,
                                        additional_dependencies: None,
                                        args: None,
                                        env: None,
                                        always_run: None,
                                        fail_fast: None,
                                        pass_filenames: None,
                                        description: None,
                                        language_version: Some(
                                            "3.8",
                                        ),
                                        log_file: None,
                                        require_serial: None,
                                        priority: None,
                                        stages: None,
                                        verbose: None,
                                        minimum_prek_version: None,
                                        _unused_keys: {},
                                    },
                                },
                            ],
                            _unused_keys: {},
                        },
                    ),
                ],
                default_install_hook_types: None,
                default_language_version: None,
                default_stages: None,
                files: None,
                exclude: None,
                fail_fast: None,
                minimum_prek_version: None,
                orphan: None,
                _unused_keys: {},
            },
        )
        "#);
    }

    #[test]
    fn test_read_config() -> Result<()> {
        let config = read_config(Path::new("tests/fixtures/uv-pre-commit-config.yaml"))?;
        insta::assert_debug_snapshot!(config);
        Ok(())
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
        let result = serde_yaml::from_str::<Config>(yaml);
        assert!(result.is_ok());
        let config = result.unwrap();
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
        let result = serde_yaml::from_str::<Config>(yaml);
        assert!(result.is_ok());
        let config = result.unwrap();
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
        let result = serde_yaml::from_str::<Config>(yaml);
        assert!(result.is_err());

        // Test that valid minimum_prek_version field works in hook config
        let yaml = indoc::indoc! {r"
          - repo: local
            hooks:
              - id: test-hook
                name: Test Hook
                entry: echo test
                language: system
                minimum_prek_version: '10.0.0'
        "};
        let result = serde_yaml::from_str::<Manifest>(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn test_validate_type_tags() {
        // Valid tags should parse successfully
        let yaml_valid = r"
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
        ";
        let result = serde_yaml::from_str::<Config>(yaml_valid);
        assert!(result.is_ok(), "Should parse valid tags successfully");

        // Empty lists and missing keys should also be fine
        let yaml_empty = r"
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
        ";
        let result_empty = serde_yaml::from_str::<Config>(yaml_empty);
        assert!(
            result_empty.is_ok(),
            "Should parse empty/missing tags successfully"
        );

        // Invalid tag in 'types' should fail
        let yaml_invalid_types = r"
            repos:
              - repo: local
                hooks:
                  - id: my-hook
                    name: My Hook
                    entry: echo
                    language: system
                    types: [pythoon] # Deliberate typo
        ";
        let result_invalid_types = serde_yaml::from_str::<Config>(yaml_invalid_types);
        assert!(result_invalid_types.is_err());

        assert!(
            result_invalid_types
                .unwrap_err()
                .to_string()
                .contains("Type tag \"pythoon\" is not recognized")
        );

        // Invalid tag in 'types_or' should fail
        let yaml_invalid_types_or = r"
            repos:
              - repo: local
                hooks:
                  - id: my-hook
                    name: My Hook
                    entry: echo
                    language: system
                    types_or: [invalidtag]
        ";
        let result_invalid_types_or = serde_yaml::from_str::<Config>(yaml_invalid_types_or);
        assert!(result_invalid_types_or.is_err());
        assert!(
            result_invalid_types_or
                .unwrap_err()
                .to_string()
                .contains("Type tag \"invalidtag\" is not recognized")
        );

        // Invalid tag in 'exclude_types' should fail
        let yaml_invalid_exclude_types = r"
            repos:
              - repo: local
                hooks:
                  - id: my-hook
                    name: My Hook
                    entry: echo
                    language: system
                    exclude_types: [not-a-real-tag]
        ";
        let result_invalid_exclude_types =
            serde_yaml::from_str::<Config>(yaml_invalid_exclude_types);
        assert!(result_invalid_exclude_types.is_err());
        assert!(
            result_invalid_exclude_types
                .unwrap_err()
                .to_string()
                .contains("Type tag \"not-a-real-tag\" is not recognized")
        );
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
        insta::assert_debug_snapshot!(config, @r#"
        Config {
            repos: [
                Local(
                    LocalRepo {
                        repo: "local",
                        hooks: [
                            ManifestHook {
                                id: "mypy-local",
                                name: "Local mypy",
                                entry: "python tools/pre_commit/mypy.py 0 \"local\"",
                                language: Python,
                                options: HookOptions {
                                    alias: None,
                                    files: None,
                                    exclude: None,
                                    types: None,
                                    types_or: Some(
                                        [
                                            "python",
                                            "pyi",
                                        ],
                                    ),
                                    exclude_types: None,
                                    additional_dependencies: None,
                                    args: None,
                                    env: None,
                                    always_run: None,
                                    fail_fast: None,
                                    pass_filenames: None,
                                    description: None,
                                    language_version: None,
                                    log_file: None,
                                    require_serial: None,
                                    priority: None,
                                    stages: None,
                                    verbose: None,
                                    minimum_prek_version: None,
                                    _unused_keys: {},
                                },
                            },
                            ManifestHook {
                                id: "mypy-3.10",
                                name: "Mypy 3.10",
                                entry: "python tools/pre_commit/mypy.py 1 \"3.10\"",
                                language: Python,
                                options: HookOptions {
                                    alias: None,
                                    files: None,
                                    exclude: None,
                                    types: None,
                                    types_or: Some(
                                        [
                                            "python",
                                            "pyi",
                                        ],
                                    ),
                                    exclude_types: None,
                                    additional_dependencies: None,
                                    args: None,
                                    env: None,
                                    always_run: None,
                                    fail_fast: None,
                                    pass_filenames: None,
                                    description: None,
                                    language_version: None,
                                    log_file: None,
                                    require_serial: None,
                                    priority: None,
                                    stages: None,
                                    verbose: None,
                                    minimum_prek_version: None,
                                    _unused_keys: {},
                                },
                            },
                        ],
                        _unused_keys: {},
                    },
                ),
            ],
            default_install_hook_types: None,
            default_language_version: None,
            default_stages: None,
            files: None,
            exclude: None,
            fail_fast: None,
            minimum_prek_version: None,
            orphan: None,
            _unused_keys: {},
        }
        "#);

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
        insta::assert_debug_snapshot!(config, @r#"
        Config {
            repos: [
                Local(
                    LocalRepo {
                        repo: "local",
                        hooks: [
                            ManifestHook {
                                id: "test-yaml",
                                name: "Test YAML compatibility",
                                entry: "prek --help",
                                language: System,
                                options: HookOptions {
                                    alias: None,
                                    files: None,
                                    exclude: None,
                                    types: None,
                                    types_or: None,
                                    exclude_types: None,
                                    additional_dependencies: None,
                                    args: None,
                                    env: None,
                                    always_run: None,
                                    fail_fast: None,
                                    pass_filenames: Some(
                                        false,
                                    ),
                                    description: None,
                                    language_version: None,
                                    log_file: None,
                                    require_serial: Some(
                                        true,
                                    ),
                                    priority: None,
                                    stages: Some(
                                        [
                                            PreCommit,
                                        ],
                                    ),
                                    verbose: None,
                                    minimum_prek_version: None,
                                    _unused_keys: {},
                                },
                            },
                        ],
                        _unused_keys: {},
                    },
                ),
            ],
            default_install_hook_types: None,
            default_language_version: None,
            default_stages: None,
            files: None,
            exclude: None,
            fail_fast: None,
            minimum_prek_version: None,
            orphan: None,
            _unused_keys: {
                "local": Object {
                    "language": String("system"),
                    "pass_filenames": Bool(false),
                    "require_serial": Bool(true),
                },
                "local-commit": Object {
                    "language": String("system"),
                    "pass_filenames": Bool(false),
                    "require_serial": Bool(true),
                    "stages": Array [
                        String("pre-commit"),
                    ],
                },
            },
        }
        "#);

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
        let result = serde_yaml::from_str::<Config>(yaml);
        assert!(result.is_ok());
    }
}

#[cfg(unix)]
#[cfg(all(test, feature = "schemars"))]
mod _gen {
    use crate::config::Config;
    use anyhow::bail;
    use prek_consts::env_vars::EnvVars;
    use pretty_assertions::StrComparison;
    use std::path::PathBuf;

    const ROOT_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../../");

    enum Mode {
        /// Update the content.
        Write,

        /// Don't write to the file, check if the file is up-to-date and error if not.
        Check,

        /// Write the generated help to stdout.
        DryRun,
    }

    fn generate() -> String {
        let settings = schemars::generate::SchemaSettings::draft07();
        let generator = schemars::SchemaGenerator::new(settings);
        let schema = generator.into_root_schema_for::<Config>();

        serde_json::to_string_pretty(&schema).unwrap() + "\n"
    }

    #[test]
    fn generate_json_schema() -> anyhow::Result<()> {
        let mode = if EnvVars::is_set(EnvVars::PREK_GENERATE) {
            Mode::Write
        } else {
            Mode::Check
        };

        let schema_string = generate();
        let filename = "prek.schema.json";
        let schema_path = PathBuf::from(ROOT_DIR).join(filename);

        match mode {
            Mode::DryRun => {
                anstream::println!("{schema_string}");
            }
            Mode::Check => match fs_err::read_to_string(schema_path) {
                Ok(current) => {
                    if current == schema_string {
                        anstream::println!("Up-to-date: {filename}");
                    } else {
                        let comparison = StrComparison::new(&current, &schema_string);
                        bail!("{filename} changed, please run `mise run generate`:\n{comparison}");
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    bail!("{filename} not found, please run `mise run generate`");
                }
                Err(err) => {
                    bail!("{filename} changed, please run `mise run generate`:\n{err}");
                }
            },
            Mode::Write => match fs_err::read_to_string(&schema_path) {
                Ok(current) => {
                    if current == schema_string {
                        anstream::println!("Up-to-date: {filename}");
                    } else {
                        anstream::println!("Updating: {filename}");
                        fs_err::write(schema_path, schema_string.as_bytes())?;
                    }
                }
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                    anstream::println!("Updating: {filename}");
                    fs_err::write(schema_path, schema_string.as_bytes())?;
                }
                Err(err) => {
                    bail!(
                        "{filename} changed, please run `cargo dev generate-cli-reference`:\n{err}"
                    );
                }
            },
        }

        Ok(())
    }
}
