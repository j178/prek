use std::collections::HashMap;
use std::fmt::Display;
use std::ops::RangeInclusive;
use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use serde::{Deserialize, Deserializer, Serialize};
use url::Url;

use crate::fs::Simplified;

pub const CONFIG_FILE: &str = ".pre-commit-config.yaml";
pub const MANIFEST_FILE: &str = ".pre-commit-hooks.yaml";

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash, Deserialize)]
#[serde(rename_all = "snake_case")]
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
    Script,
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
pub enum HookType {
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

// TODO: warn on deprecated stages
#[derive(Debug, Clone, Copy, PartialEq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "kebab-case")]
pub enum Stage {
    Manual,
    CommitMsg,
    PostCheckout,
    PostCommit,
    PostMerge,
    PostRewrite,
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

// TODO: warn unexpected keys
// TODO: warn deprecated stage
// TODO: warn sensible regex
// TODO: check minimum_pre_commit_version
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ConfigWire {
    pub repos: Vec<ConfigRepo>,
    /// A list of --hook-types which will be used by default when running pre-commit install.
    /// Default is `[pre-commit]`.
    pub default_install_hook_types: Option<Vec<HookType>>,
    /// A mapping from language to the default `language_version`.
    pub default_language_version: Option<HashMap<Language, String>>,
    /// A configuration-wide default for the stages property of hooks.
    /// Default to all stages.
    pub default_stages: Option<Vec<Stage>>,
    /// Global file include pattern.
    pub files: Option<String>,
    /// Global file exclude pattern.
    pub exclude: Option<String>,
    /// Set to true to have pre-commit stop running hooks after the first failure.
    /// Default is false.
    pub fail_fast: Option<bool>,
    pub minimum_pre_commit_version: Option<String>,
    /// Configuration for pre-commit.ci service.
    pub ci: Option<HashMap<String, serde_yaml::Value>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RepoLocation {
    Local,
    Meta,
    Remote(Url),
}

impl FromStr for RepoLocation {
    type Err = url::ParseError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "local" => Ok(RepoLocation::Local),
            "meta" => Ok(RepoLocation::Meta),
            _ => Url::parse(s).map(RepoLocation::Remote),
        }
    }
}

impl<'de> Deserialize<'de> for RepoLocation {
    fn deserialize<D>(deserializer: D) -> Result<RepoLocation, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        RepoLocation::from_str(&s).map_err(serde::de::Error::custom)
    }
}

impl RepoLocation {
    pub fn as_str(&self) -> &str {
        match self {
            RepoLocation::Local => "local",
            RepoLocation::Meta => "meta",
            RepoLocation::Remote(_) => "remote",
        }
    }
}

impl Display for RepoLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A remote hook in the configuration file.
///
/// All keys in manifest hook dict are valid in a config hook dict, but are optional.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ConfigRemoteHook {
    /// The id of the hook.
    pub id: String,
    /// Override the name of the hook.
    pub name: Option<String>,
    /// Not documented in the official docs.
    pub entry: Option<String>,
    /// Not documented in the official docs.
    pub language: Option<Language>,
    /// Allows the hook to be referenced using an additional id when using pre-commit run <hookid>
    pub alias: Option<String>,
    /// Override the pattern of files to run on.
    pub files: Option<String>,
    /// Override the pattern of files to exclude.
    pub exclude: Option<String>,
    /// Override the types of files to run on (AND).
    pub types: Option<Vec<String>>,
    /// Override the types of files to run on (OR).
    pub types_or: Option<Vec<String>>,
    /// Override the types of files to exclude.
    pub exclude_types: Option<Vec<String>>,
    /// Additional dependencies to install in the environment where the hook runs.
    pub additional_dependencies: Option<Vec<String>>,
    /// Additional arguments to pass to the hook.
    pub args: Option<Vec<String>>,
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
    pub minimum_pre_commit_version: Option<String>,
}

/// A local hook in the configuration file.
///
/// It's the same as the manifest hook definition.
pub type ConfigLocalHook = ManifestHook;

#[derive(Debug, Copy, Clone, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum MetaHookID {
    CheckHooksApply,
    CheckUselessExcludes,
    Identify,
}

impl MetaHookID {
    pub fn as_str(&self) -> &str {
        match self {
            MetaHookID::CheckHooksApply => "check-hooks-apply",
            MetaHookID::CheckUselessExcludes => "check-useless-excludes",
            MetaHookID::Identify => "identify",
        }
    }
}

impl Display for MetaHookID {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(deny_unknown_fields)]
pub struct ConfigMetaHook {
    pub id: MetaHookID,
    // only "system" is allowed
    pub language: Option<Language>,
    // TODO: entry is not allowed
}

#[derive(Debug, Clone)]
pub struct ConfigRemoteRepo {
    pub repo: Url,
    pub rev: String,
    pub hooks: Vec<ConfigRemoteHook>,
}

#[derive(Debug, Clone)]
pub struct ConfigLocalRepo {
    pub repo: String,
    pub hooks: Vec<ConfigLocalHook>,
}

#[derive(Debug, Clone)]
pub struct ConfigMetaRepo {
    pub repo: String,
    pub hooks: Vec<ConfigMetaHook>,
}

#[derive(Debug, Clone)]
pub enum ConfigRepo {
    Remote(ConfigRemoteRepo),
    Local(ConfigLocalRepo),
    Meta(ConfigMetaRepo),
}

impl<'de> Deserialize<'de> for ConfigRepo {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct RepoWire {
            repo: RepoLocation,
            #[serde(flatten)]
            rest: serde_yaml::Value,
        }

        let RepoWire { repo, rest } = RepoWire::deserialize(deserializer)?;

        match repo {
            RepoLocation::Remote(url) => {
                #[derive(Deserialize)]
                struct RemoteRepo {
                    rev: String,
                    hooks: Vec<ConfigRemoteHook>,
                }
                let RemoteRepo { rev, hooks } = RemoteRepo::deserialize(rest)
                    .map_err(|e| serde::de::Error::custom(format!("Invalid remote repo: {e}")))?;

                Ok(ConfigRepo::Remote(ConfigRemoteRepo {
                    repo: url,
                    rev,
                    hooks,
                }))
            }
            RepoLocation::Local => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct LocalRepo {
                    hooks: Vec<ConfigLocalHook>,
                }
                let LocalRepo { hooks } = LocalRepo::deserialize(rest)
                    .map_err(|e| serde::de::Error::custom(format!("Invalid local repo: {e}")))?;
                Ok(ConfigRepo::Local(ConfigLocalRepo {
                    repo: "local".to_string(),
                    hooks,
                }))
            }
            RepoLocation::Meta => {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct MetaRepo {
                    hooks: Vec<ConfigMetaHook>,
                }
                let MetaRepo { hooks } = MetaRepo::deserialize(rest)
                    .map_err(|e| serde::de::Error::custom(format!("Invalid meta repo: {e}")))?;
                Ok(ConfigRepo::Meta(ConfigMetaRepo {
                    repo: "meta".to_string(),
                    hooks,
                }))
            }
        }
    }
}

impl ConfigRepo {
    pub fn hook_ids(&self) -> Vec<&str> {
        match self {
            ConfigRepo::Remote(repo) => repo.hooks.iter().map(|h| h.id.as_str()).collect(),
            ConfigRepo::Local(repo) => repo.hooks.iter().map(|h| h.id.as_str()).collect(),
            ConfigRepo::Meta(repo) => repo.hooks.iter().map(|h| h.id.as_str()).collect(),
        }
    }
}

// TODO: check minimum_pre_commit_version

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct ManifestHook {
    /// The id of the hook.
    pub id: String,
    /// The name of the hook.
    pub name: String,
    /// The command to run. It can contain arguments that will not be overridden.
    pub entry: String,
    /// The language of the hook. Tells pre-commit how to install and run the hook.
    pub language: Language,
    /// Not documented in the official docs.
    pub alias: Option<String>,
    /// The pattern of files to run on.
    pub files: Option<String>,
    /// Exclude files that were matched by `files`.
    /// Default is `$^`, which matches nothing.
    pub exclude: Option<String>,
    /// List of file types to run on (AND).
    /// Default is `[file]`, which matches all files.
    pub types: Option<Vec<String>>,
    /// List of file types to run on (OR).
    /// Default is `[]`.
    pub types_or: Option<Vec<String>>,
    /// List of file types to exclude.
    /// Default is `[]`.
    pub exclude_types: Option<Vec<String>>,
    /// Not documented in the official docs.
    pub additional_dependencies: Option<Vec<String>>,
    /// Additional arguments to pass to the hook.
    pub args: Option<Vec<String>>,
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
    pub minimum_pre_commit_version: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[serde(transparent)]
pub struct ManifestWire {
    pub hooks: Vec<ManifestHook>,
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("Config file not found: {0}")]
    NotFound(String),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("Failed to parse `{0}`")]
    Yaml(String, #[source] serde_yaml::Error),

    #[error("Invalid repo URL: {0}")]
    RepoUrl(#[from] url::ParseError),
}

/// Read the configuration file from the given path.
pub fn read_config(path: &Path) -> Result<ConfigWire, Error> {
    let content = match fs_err::read_to_string(path) {
        Ok(content) => content,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(Error::NotFound(path.user_display().to_string()));
        }
        Err(e) => return Err(e.into()),
    };
    let config = serde_yaml::from_str(&content)
        .map_err(|e| Error::Yaml(path.user_display().to_string(), e))?;
    Ok(config)
}

// TODO: check id duplication?
/// Read the manifest file from the given path.
pub fn read_manifest(path: &Path) -> Result<ManifestWire, Error> {
    let content = fs_err::read_to_string(path)?;
    let manifest = serde_yaml::from_str(&content)
        .map_err(|e| Error::Yaml(path.user_display().to_string(), e))?;
    Ok(manifest)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let result = serde_yaml::from_str::<ConfigWire>(yaml);
        insta::assert_debug_snapshot!(result, @r###"
        Ok(
            ConfigWire {
                repos: [
                    Local(
                        ConfigLocalRepo {
                            repo: "local",
                            hooks: [
                                ManifestHook {
                                    id: "cargo-fmt",
                                    name: "cargo fmt",
                                    entry: "cargo fmt --",
                                    language: System,
                                    alias: None,
                                    files: None,
                                    exclude: None,
                                    types: None,
                                    types_or: None,
                                    exclude_types: None,
                                    additional_dependencies: None,
                                    args: None,
                                    always_run: None,
                                    fail_fast: None,
                                    pass_filenames: None,
                                    description: None,
                                    language_version: None,
                                    log_file: None,
                                    require_serial: None,
                                    stages: None,
                                    verbose: None,
                                    minimum_pre_commit_version: None,
                                },
                            ],
                        },
                    ),
                ],
                default_install_hook_types: None,
                default_language_version: None,
                default_stages: None,
                files: None,
                exclude: None,
                fail_fast: None,
                minimum_pre_commit_version: None,
                ci: None,
            },
        )
        "###);

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
        let result = serde_yaml::from_str::<ConfigWire>(yaml);
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Error("repos: Invalid local repo: unknown field `rev`, expected `hooks`", line: 2, column: 3),
        )
        "###);

        // Remote hook should have `rev`.
        let yaml = indoc::indoc! {r"
            repos:
              - repo: https://github.com/crate-ci/typos
                rev: v1.0.0
                hooks:
                  - id: typos
        "};
        let result = serde_yaml::from_str::<ConfigWire>(yaml);
        insta::assert_debug_snapshot!(result, @r###"
        Ok(
            ConfigWire {
                repos: [
                    Remote(
                        ConfigRemoteRepo {
                            repo: Url {
                                scheme: "https",
                                cannot_be_a_base: false,
                                username: "",
                                password: None,
                                host: Some(
                                    Domain(
                                        "github.com",
                                    ),
                                ),
                                port: None,
                                path: "/crate-ci/typos",
                                query: None,
                                fragment: None,
                            },
                            rev: "v1.0.0",
                            hooks: [
                                ConfigRemoteHook {
                                    id: "typos",
                                    name: None,
                                    entry: None,
                                    language: None,
                                    alias: None,
                                    files: None,
                                    exclude: None,
                                    types: None,
                                    types_or: None,
                                    exclude_types: None,
                                    additional_dependencies: None,
                                    args: None,
                                    always_run: None,
                                    fail_fast: None,
                                    pass_filenames: None,
                                    description: None,
                                    language_version: None,
                                    log_file: None,
                                    require_serial: None,
                                    stages: None,
                                    verbose: None,
                                    minimum_pre_commit_version: None,
                                },
                            ],
                        },
                    ),
                ],
                default_install_hook_types: None,
                default_language_version: None,
                default_stages: None,
                files: None,
                exclude: None,
                fail_fast: None,
                minimum_pre_commit_version: None,
                ci: None,
            },
        )
        "###);

        let yaml = indoc::indoc! {r"
            repos:
              - repo: https://github.com/crate-ci/typos
                hooks:
                  - id: typos
        "};
        let result = serde_yaml::from_str::<ConfigWire>(yaml);
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Error("repos: Invalid remote repo: missing field `rev`", line: 2, column: 3),
        )
        "###);
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
        let result = serde_yaml::from_str::<ConfigWire>(yaml);
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Error("repos: Invalid remote repo: missing field `id`", line: 2, column: 3),
        )
        "###);

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
        let result = serde_yaml::from_str::<ConfigWire>(yaml);
        insta::assert_debug_snapshot!(result, @r###"
        Err(
            Error("repos: Invalid local repo: missing field `language`", line: 2, column: 3),
        )
        "###);

        let yaml = indoc::indoc! { r"
            repos:
              - repo: local
                hooks:
                  - id: cargo-fmt
                    name: cargo fmt
                    entry: cargo fmt
                    language: rust
        "};
        let result = serde_yaml::from_str::<ConfigWire>(yaml);
        insta::assert_debug_snapshot!(result, @r###"
        Ok(
            ConfigWire {
                repos: [
                    Local(
                        ConfigLocalRepo {
                            repo: "local",
                            hooks: [
                                ManifestHook {
                                    id: "cargo-fmt",
                                    name: "cargo fmt",
                                    entry: "cargo fmt",
                                    language: Rust,
                                    alias: None,
                                    files: None,
                                    exclude: None,
                                    types: None,
                                    types_or: None,
                                    exclude_types: None,
                                    additional_dependencies: None,
                                    args: None,
                                    always_run: None,
                                    fail_fast: None,
                                    pass_filenames: None,
                                    description: None,
                                    language_version: None,
                                    log_file: None,
                                    require_serial: None,
                                    stages: None,
                                    verbose: None,
                                    minimum_pre_commit_version: None,
                                },
                            ],
                        },
                    ),
                ],
                default_install_hook_types: None,
                default_language_version: None,
                default_stages: None,
                files: None,
                exclude: None,
                fail_fast: None,
                minimum_pre_commit_version: None,
                ci: None,
            },
        )
        "###);
    }

    #[test]
    fn test_read_config() -> Result<()> {
        let config = read_config(Path::new("tests/files/uv-pre-commit-config.yaml"))?;
        insta::assert_debug_snapshot!(config);
        Ok(())
    }

    #[test]
    fn test_read_manifest() -> Result<()> {
        let manifest = read_manifest(Path::new("tests/files/uv-pre-commit-hooks.yaml"))?;
        insta::assert_debug_snapshot!(manifest);
        Ok(())
    }
}
