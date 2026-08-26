use std::collections::BTreeMap;
use std::fmt::Display;

use anyhow::Result;
use itertools::Itertools;
use prek_identify::TagSet;
use rustc_hash::FxHashMap;
use serde::de::Error as DeError;
use serde::{Deserialize, Deserializer, Serialize};
use strum::EnumCount;

use super::deserialize_and_validate_minimum_version;
use super::pattern::FilePattern;
use super::priority::{Priority, deserialize_groups};

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
    Deno,
    Docker,
    DockerImage,
    Dotnet,
    Fail,
    Golang,
    Haskell,
    Julia,
    Lua,
    Mise,
    Node,
    Perl,
    Php,
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
    strum::EnumCount,
)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[repr(u8)]
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
    const ORDER: [Self; Self::COUNT] = [
        Self::Manual,
        Self::CommitMsg,
        Self::PostCheckout,
        Self::PostCommit,
        Self::PostMerge,
        Self::PostRewrite,
        Self::PreCommit,
        Self::PreMergeCommit,
        Self::PrePush,
        Self::PreRebase,
        Self::PrepareCommitMsg,
    ];

    const fn bit(self) -> u16 {
        1u16 << (self as u8)
    }

    fn from_index(index: u32) -> Self {
        Self::ORDER[index as usize]
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Stages(u16);

impl Stages {
    const ALL_BITS: u16 = (1u16 << Stage::COUNT) - 1;

    pub(crate) const ALL: Self = Self(Self::ALL_BITS);

    pub(crate) fn iter(self) -> impl Iterator<Item = Stage> {
        let mut bits = self.0;
        std::iter::from_fn(move || {
            if bits == 0 {
                return None;
            }
            let index = bits.trailing_zeros();
            bits &= bits - 1;
            Some(Stage::from_index(index))
        })
    }

    pub(crate) fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub(crate) fn contains(self, stage: Stage) -> bool {
        (self.0 & stage.bit()) != 0
    }
}

impl Display for Stages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if *self == Self::ALL {
            write!(f, "all")
        } else if self.is_empty() {
            write!(f, "none")
        } else {
            let stages_str = self.iter().map(|stage| stage.to_string()).join(", ");
            write!(f, "{stages_str}")
        }
    }
}

impl std::fmt::Debug for Stages {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Stages({self})")
    }
}

impl From<Vec<Stage>> for Stages {
    fn from(value: Vec<Stage>) -> Self {
        let bits = value.into_iter().fold(0, |bits, stage| bits | stage.bit());
        Self(bits & Self::ALL_BITS)
    }
}

impl<const N: usize> From<[Stage; N]> for Stages {
    fn from(value: [Stage; N]) -> Self {
        Self::from(Vec::from(value))
    }
}

impl<'de> Deserialize<'de> for Stages {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let stages = Vec::<Stage>::deserialize(deserializer)?;
        Ok(Self::from(stages))
    }
}

/// Controls whether filenames are appended to a hook's command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PassFilenames {
    /// Pass all matching filenames (default). Corresponds to `pass_filenames:
    /// true`.
    All,
    /// Pass no filenames. Corresponds to `pass_filenames: false`.
    None,
    /// Pass at most `n` filenames per invocation. Corresponds to
    /// `pass_filenames: n`.
    Limited(std::num::NonZeroUsize),
}

impl<'de> Deserialize<'de> for PassFilenames {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct PassFilenamesVisitor;

        impl serde::de::Visitor<'_> for PassFilenamesVisitor {
            type Value = PassFilenames;

            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a boolean or a positive integer")
            }

            fn visit_bool<E: DeError>(self, v: bool) -> Result<PassFilenames, E> {
                Ok(if v {
                    PassFilenames::All
                } else {
                    PassFilenames::None
                })
            }

            fn visit_u64<E: DeError>(self, v: u64) -> Result<PassFilenames, E> {
                let n = usize::try_from(v)
                    .ok()
                    .and_then(std::num::NonZeroUsize::new)
                    .ok_or_else(|| {
                        E::custom(
                            "pass_filenames must be a positive integer; use `false` to pass no filenames",
                        )
                    })?;
                Ok(PassFilenames::Limited(n))
            }

            fn visit_i64<E: DeError>(self, v: i64) -> Result<PassFilenames, E> {
                if v <= 0 {
                    return Err(E::custom(
                        "pass_filenames must be a positive integer; use `false` to pass no filenames",
                    ));
                }
                #[allow(clippy::cast_sign_loss)]
                self.visit_u64(v as u64)
            }
        }

        deserializer.deserialize_any(PassFilenamesVisitor)
    }
}

/// A predefined shell adapter used to run hook entries as shell source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(rename_all = "lowercase"))]
pub(crate) enum Shell {
    Sh,
    Bash,
    Pwsh,
    Powershell,
    Cmd,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, strum::Display)]
#[serde(rename_all = "kebab-case")]
#[strum(serialize_all = "kebab-case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub(crate) enum ToolchainPreference {
    OnlyManaged,
    #[default]
    Managed,
    System,
    OnlySystem,
}

#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(rename = "LanguageVersion"))]
enum LanguageVersionWire {
    Request(String),
    Options {
        /// A language-specific version request. Defaults to `default`.
        request: Option<String>,
        /// Which toolchain sources to prefer or require. Defaults to `managed`.
        preference: Option<ToolchainPreference>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum ConfiguredVersionRequest {
    Unspecified,
    Default,
    Explicit(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(from = "LanguageVersionWire")]
#[cfg_attr(
    feature = "schemars",
    derive(schemars::JsonSchema),
    schemars(with = "LanguageVersionWire")
)]
pub(crate) struct LanguageVersion {
    request: ConfiguredVersionRequest,
    preference: Option<ToolchainPreference>,
    allows_download: bool,
}

impl LanguageVersion {
    pub(crate) fn request(&self) -> Option<&str> {
        match &self.request {
            ConfiguredVersionRequest::Explicit(request) => Some(request),
            ConfiguredVersionRequest::Unspecified | ConfiguredVersionRequest::Default => None,
        }
    }

    pub(crate) fn preference(&self) -> ToolchainPreference {
        self.preference.unwrap_or_default()
    }

    pub(crate) fn allows_download(&self) -> bool {
        self.allows_download && self.preference() != ToolchainPreference::OnlySystem
    }

    pub(crate) fn update(&mut self, other: &Self) {
        if other.request != ConfiguredVersionRequest::Unspecified {
            self.request.clone_from(&other.request);
            self.allows_download = other.allows_download;
        }
        if other.preference.is_some() {
            self.preference = other.preference;
        }
    }

    pub(crate) fn apply_defaults(&mut self, defaults: &Self) {
        if self.request == ConfiguredVersionRequest::Unspecified {
            self.request.clone_from(&defaults.request);
            self.allows_download = defaults.allows_download;
        }
        if self.preference.is_none() {
            self.preference = defaults.preference;
        }
    }

    pub(crate) fn is_default(&self) -> bool {
        self.preference.is_none()
            && matches!(
                self.request,
                ConfiguredVersionRequest::Unspecified | ConfiguredVersionRequest::Default
            )
            && self.allows_download
    }
}

impl From<String> for LanguageVersion {
    fn from(request: String) -> Self {
        Self::from(LanguageVersionWire::Request(request))
    }
}

impl From<&str> for LanguageVersion {
    fn from(request: &str) -> Self {
        Self::from(request.to_string())
    }
}

impl From<LanguageVersionWire> for LanguageVersion {
    fn from(wire: LanguageVersionWire) -> Self {
        let (request, preference) = match wire {
            LanguageVersionWire::Request(request) => (Some(request), None),
            LanguageVersionWire::Options {
                request,
                preference,
            } => (request, preference),
        };
        let (request, allows_download) = match request {
            Some(request) if request == "default" => (ConfiguredVersionRequest::Default, true),
            Some(request) if request == "system" => (ConfiguredVersionRequest::Default, false),
            Some(request) => (ConfiguredVersionRequest::Explicit(request), true),
            None => (ConfiguredVersionRequest::Unspecified, true),
        };

        Self {
            request,
            preference,
            allows_download,
        }
    }
}

impl Display for LanguageVersion {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let request = match &self.request {
            ConfiguredVersionRequest::Unspecified => None,
            ConfiguredVersionRequest::Default if self.allows_download => Some("default"),
            ConfiguredVersionRequest::Default => Some("system"),
            ConfiguredVersionRequest::Explicit(request) => Some(request.as_str()),
        };

        if self.preference.is_none()
            && let Some(request) = request
        {
            return f.write_str(request);
        }

        f.write_str("{")?;
        if let Some(request) = request {
            write!(f, " request: {request}")?;
        }
        if let Some(preference) = self.preference {
            if request.is_some() {
                f.write_str(",")?;
            }
            write!(f, " preference: {preference}")?;
        }
        f.write_str(" }")
    }
}

/// Common hook options.
#[derive(Debug, Clone, Default)]
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
    pub types: Option<TagSet>,
    /// List of file types to run on (OR).
    /// Default is `[]`.
    pub types_or: Option<TagSet>,
    /// List of file types to exclude.
    /// Default is `[]`.
    pub exclude_types: Option<TagSet>,
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
    pub pass_filenames: Option<PassFilenames>,
    /// A description of the hook.
    pub description: Option<String>,
    /// Select a language version and optionally configure the toolchain source preference.
    /// Default is `default`.
    /// See <https://pre-commit.com/#overriding-language-version>.
    pub language_version: Option<LanguageVersion>,
    /// Write the output of the hook to a file when the hook fails or verbose is enabled.
    pub log_file: Option<String>,
    /// Run the hook entry through a predefined shell adapter.
    pub shell: Option<Shell>,
    /// This hook will execute using a single process instead of in parallel.
    /// Default is false.
    pub require_serial: Option<bool>,
    /// Select which Git hook stages this hook runs for.
    /// Default all stages are selected.
    /// See <https://pre-commit.com/#confining-hooks-to-run-at-certain-stages>.
    pub stages: Option<Stages>,
    /// Print the output of the hook even if it passes.
    /// Default is false.
    pub verbose: Option<bool>,
    /// The minimum version of prek required to run this hook.
    pub minimum_prek_version: Option<String>,

    #[cfg_attr(feature = "schemars", schemars(skip))]
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
            log_file,
            shell,
            require_serial,
            stages,
            verbose,
            minimum_prek_version,
        );

        if let Some(other_language_version) = &other.language_version {
            if let Some(language_version) = &mut self.language_version {
                language_version.update(other_language_version);
            } else {
                self.language_version.clone_from(&other.language_version);
            }
        }

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

/// Flat serde representation for all hook kinds.
///
/// Keeping the option fields at this level preserves source spans that serde's
/// nested `flatten` buffering would otherwise discard.
#[derive(Debug, Deserialize)]
struct HookWire {
    id: String,
    name: Option<String>,
    entry: Option<String>,
    language: Option<Language>,
    priority: Option<Priority>,
    #[serde(default, deserialize_with = "deserialize_groups")]
    groups: Option<Vec<String>>,
    alias: Option<String>,
    files: Option<FilePattern>,
    exclude: Option<FilePattern>,
    types: Option<TagSet>,
    types_or: Option<TagSet>,
    exclude_types: Option<TagSet>,
    additional_dependencies: Option<Vec<String>>,
    args: Option<Vec<String>>,
    env: Option<FxHashMap<String, String>>,
    always_run: Option<bool>,
    fail_fast: Option<bool>,
    pass_filenames: Option<PassFilenames>,
    description: Option<String>,
    language_version: Option<LanguageVersion>,
    log_file: Option<String>,
    shell: Option<Shell>,
    require_serial: Option<bool>,
    stages: Option<Stages>,
    verbose: Option<bool>,
    #[serde(deserialize_with = "deserialize_and_validate_minimum_version", default)]
    minimum_prek_version: Option<String>,
    #[serde(flatten)]
    _unused_keys: BTreeMap<String, serde_json::Value>,
}

impl HookWire {
    fn take_options(&mut self) -> HookOptions {
        HookOptions {
            alias: self.alias.take(),
            files: self.files.take(),
            exclude: self.exclude.take(),
            types: self.types.take(),
            types_or: self.types_or.take(),
            exclude_types: self.exclude_types.take(),
            additional_dependencies: self.additional_dependencies.take(),
            args: self.args.take(),
            env: self.env.take(),
            always_run: self.always_run.take(),
            fail_fast: self.fail_fast.take(),
            pass_filenames: self.pass_filenames.take(),
            description: self.description.take(),
            language_version: self.language_version.take(),
            log_file: self.log_file.take(),
            shell: self.shell.take(),
            require_serial: self.require_serial.take(),
            stages: self.stages.take(),
            verbose: self.verbose.take(),
            minimum_prek_version: self.minimum_prek_version.take(),
            _unused_keys: std::mem::take(&mut self._unused_keys),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RemoteHook", rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(!try_from))]
pub(crate) struct ManifestHook {
    /// The id of the hook.
    pub id: String,
    /// The name of the hook.
    pub name: String,
    /// The command to run. It can contain arguments that will not be overridden.
    pub entry: String,
    /// The language of the hook. Tells prek how to install and run the hook.
    pub language: Language,
    #[cfg_attr(feature = "schemars", serde(flatten))]
    pub options: HookOptions,
}

impl TryFrom<RemoteHook> for ManifestHook {
    type Error = serde::de::value::Error;

    fn try_from(hook: RemoteHook) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            id: hook.id,
            name: hook
                .name
                .ok_or_else(|| Self::Error::missing_field("name"))?,
            entry: hook
                .entry
                .ok_or_else(|| Self::Error::missing_field("entry"))?,
            language: hook
                .language
                .ok_or_else(|| Self::Error::missing_field("language"))?,
            options: hook.options,
        })
    }
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
#[serde(from = "HookWire", rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(!from))]
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
    pub priority: Option<Priority>,
    /// User-defined hook groups used by `prek run --group` and `--no-group`.
    /// Group names cannot be empty, contain whitespace, or start with `@`.
    #[cfg_attr(feature = "schemars", schemars(inner(regex(pattern = r"^[^@\s]\S*$"))))]
    pub groups: Option<Vec<String>>,
    #[cfg_attr(feature = "schemars", serde(flatten))]
    pub options: HookOptions,
}

impl From<HookWire> for RemoteHook {
    fn from(mut hook: HookWire) -> Self {
        let options = hook.take_options();
        Self {
            id: hook.id,
            name: hook.name,
            entry: hook.entry,
            language: hook.language,
            priority: hook.priority,
            groups: hook.groups,
            options,
        }
    }
}

/// A local hook in the configuration file.
///
/// This is similar to `ManifestHook`, but includes config-only fields (like `priority`).
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RemoteHook", rename_all = "snake_case")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[cfg_attr(feature = "schemars", schemars(!try_from))]
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
    pub priority: Option<Priority>,
    /// User-defined hook groups used by `prek run --group` and `--no-group`.
    /// Group names cannot be empty, contain whitespace, or start with `@`.
    #[cfg_attr(feature = "schemars", schemars(inner(regex(pattern = r"^[^@\s]\S*$"))))]
    pub groups: Option<Vec<String>>,
    #[cfg_attr(feature = "schemars", serde(flatten))]
    pub options: HookOptions,
}

impl TryFrom<RemoteHook> for LocalHook {
    type Error = serde::de::value::Error;

    fn try_from(hook: RemoteHook) -> std::result::Result<Self, Self::Error> {
        Ok(Self {
            id: hook.id,
            name: hook
                .name
                .ok_or_else(|| Self::Error::missing_field("name"))?,
            entry: hook
                .entry
                .ok_or_else(|| Self::Error::missing_field("entry"))?,
            language: hook
                .language
                .ok_or_else(|| Self::Error::missing_field("language"))?,
            priority: hook.priority,
            groups: hook.groups,
            options: hook.options,
        })
    }
}

/// A meta hook predefined in pre-commit.
///
/// It's the same as the manifest hook definition but with only a few predefined id allowed.
#[derive(Debug, Clone, Deserialize)]
#[serde(try_from = "RemoteHook")]
pub(crate) struct MetaHook {
    /// The id of the hook.
    pub id: String,
    /// The name of the hook.
    pub name: String,
    /// Priority used by the scheduler to determine ordering and concurrency.
    /// Hooks with the same priority can run in parallel.
    pub priority: Option<Priority>,
    /// User-defined hook groups used by `prek run --group` and `--no-group`.
    /// Group names cannot be empty, contain whitespace, or start with `@`.
    pub groups: Option<Vec<String>>,
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
                id: hook_options.id,
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
        if hook_options.groups.is_some() {
            meta_hook.groups = hook_options.groups;
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
    pub priority: Option<Priority>,
    /// User-defined hook groups used by `prek run --group` and `--no-group`.
    /// Group names cannot be empty, contain whitespace, or start with `@`.
    pub groups: Option<Vec<String>>,
    /// Common hook options.
    ///
    /// Builtin hooks allow the same set of options overrides as other hooks.
    pub options: HookOptions,
}

impl TryFrom<RemoteHook> for BuiltinHook {
    type Error = PredefinedHookWireError;

    fn try_from(hook_options: RemoteHook) -> Result<Self, Self::Error> {
        let mut builtin_hook = BuiltinHook::from_id(&hook_options.id).map_err(|()| {
            PredefinedHookWireError::UnknownId {
                kind: PredefinedHookKind::Builtin,
                id: hook_options.id,
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
        if hook_options.groups.is_some() {
            builtin_hook.groups = hook_options.groups;
        }
        builtin_hook.options.update(&hook_options.options);

        Ok(builtin_hook)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    fn parse_language_version(yaml: &str) -> LanguageVersion {
        serde_saphyr::from_str(yaml).expect("language version should parse")
    }

    #[test]
    fn language_version_accepts_request_and_options_forms() {
        let scalar = parse_language_version("'3.12'\n");
        assert_eq!(scalar.request(), Some("3.12"));
        assert_eq!(scalar.preference(), ToolchainPreference::Managed);

        let options =
            parse_language_version("request: '>=3.12, <3.13'\npreference: only-managed\n");
        assert_eq!(options.request(), Some(">=3.12, <3.13"));
        assert_eq!(options.preference(), ToolchainPreference::OnlyManaged);

        let options = parse_language_version("request: '3.13'\n");
        assert_eq!(options.preference(), ToolchainPreference::Managed);
    }

    #[test]
    fn language_version_special_requests_clear_request_and_set_download_option() {
        for (wire, allows_download) in [
            ("default", true),
            ("request: default\n", true),
            ("system", false),
            ("request: system\n", false),
        ] {
            let language_version = parse_language_version(wire);
            assert_eq!(
                language_version.request,
                ConfiguredVersionRequest::Default,
                "{wire}"
            );
            assert_eq!(language_version.request(), None, "{wire}");
            assert_eq!(
                language_version.allows_download(),
                allows_download,
                "{wire}"
            );
            assert_eq!(language_version.is_default(), allows_download, "{wire}");
        }
    }

    #[test]
    fn language_version_update_merges_fields() {
        let mut manifest = HookOptions {
            language_version: Some(LanguageVersion::from("system")),
            ..Default::default()
        };
        let override_options = HookOptions {
            language_version: Some(parse_language_version("preference: only-managed\n")),
            ..Default::default()
        };

        manifest.update(&override_options);

        let language_version = manifest.language_version.as_ref().unwrap();
        assert_eq!(language_version.request(), None);
        assert_eq!(
            language_version.preference(),
            ToolchainPreference::OnlyManaged
        );
        assert!(!language_version.allows_download());

        let mut manifest = HookOptions {
            language_version: Some(parse_language_version("preference: only-managed\n")),
            ..Default::default()
        };
        let override_options = HookOptions {
            language_version: Some(LanguageVersion::from("3.13")),
            ..Default::default()
        };

        manifest.update(&override_options);

        let language_version = manifest.language_version.as_ref().unwrap();
        assert_eq!(language_version.request(), Some("3.13"));
        assert_eq!(
            language_version.preference(),
            ToolchainPreference::OnlyManaged
        );
        assert!(language_version.allows_download());
    }

    #[test]
    fn language_version_special_requests_override_explicit_versions() {
        for (request, allows_download) in [("default", true), ("system", false)] {
            let mut manifest = HookOptions {
                language_version: Some(LanguageVersion::from("3.12")),
                ..Default::default()
            };
            let override_options = HookOptions {
                language_version: Some(LanguageVersion::from(request)),
                ..Default::default()
            };

            manifest.update(&override_options);

            let language_version = manifest.language_version.as_ref().unwrap();
            assert_eq!(language_version.request(), None, "{request}");
            assert_eq!(
                language_version.allows_download(),
                allows_download,
                "{request}"
            );
        }
    }

    #[test]
    fn language_version_defaults_fill_missing_fields() {
        let defaults = parse_language_version("request: system\npreference: only-managed\n");
        let mut language_version = parse_language_version("preference: system\n");

        language_version.apply_defaults(&defaults);

        assert_eq!(language_version.request(), None);
        assert_eq!(language_version.preference(), ToolchainPreference::System);
        assert!(!language_version.allows_download());
    }

    #[test]
    fn language_version_rejects_unknown_option_fields() {
        let result = serde_saphyr::from_str::<LanguageVersion>("toolchain: system\n");
        assert!(result.is_err());
    }

    #[test]
    fn stages_deserialize_empty_as_empty() {
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            stages: Stages,
        }

        let parsed: Wrapper = serde_saphyr::from_str("stages: []\n").expect("stages should parse");
        assert_eq!(parsed.stages, Stages::from([]));
        assert!(!parsed.stages.contains(Stage::Manual));
        assert!(!parsed.stages.contains(Stage::PreCommit));
    }

    #[test]
    fn stages_deserialize_to_subset() {
        #[derive(Debug, Deserialize)]
        struct Wrapper {
            stages: Stages,
        }

        let parsed: Wrapper =
            serde_saphyr::from_str("stages: [pre-commit, manual]\n").expect("stages should parse");
        assert!(parsed.stages.contains(Stage::PreCommit));
        assert!(parsed.stages.contains(Stage::Manual));
        assert!(!parsed.stages.contains(Stage::PrePush));
    }

    #[test]
    fn stages_full_set_normalizes_to_all() {
        assert_eq!(Stages::from(Stage::value_variants().to_vec()), Stages::ALL);
    }
}
