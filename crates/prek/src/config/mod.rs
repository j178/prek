mod hook;
mod pattern;
mod priority;
mod repo;
mod update;

use std::collections::BTreeMap;
use std::error::Error as _;
use std::path::Path;

use anyhow::Result;
use itertools::Itertools;
use owo_colors::OwoColorize;
use rustc_hash::FxHashMap;
use serde::{Deserialize, Deserializer};

pub(crate) use hook::Language;
pub(crate) use hook::{
    BuiltinHook, HookOptions, HookType, LanguageVersion, LocalHook, Manifest, ManifestHook,
    MetaHook, PassFilenames, RemoteHook, Shell, Stage, Stages, ToolchainPreference,
};
pub(crate) use pattern::{FilePattern, GlobPatterns};
pub(crate) use priority::{Priority, PriorityAlias, validate_name};
#[cfg(feature = "schemars")]
pub(crate) use repo::{BuiltinRepo, LocalRepo, MetaRepo};
pub(crate) use repo::{RemoteRepo, RemoteRepoKey, Repo};
pub(crate) use update::{StringOrList, UpdateOptions};

use crate::fs::Simplified;
use crate::install_source::InstallSource;
use crate::version;
use crate::warn_user;
use crate::warn_user_once;

// TODO: warn sensible regex
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(
    feature = "schemars",
    derive(schemars::JsonSchema),
    schemars(title = "prek.toml"),
    schemars(description = "The configuration file for prek, a git hook manager written in Rust."),
    schemars(extend("$id" = "https://www.schemastore.org/prek.json")),
    schemars(extend("x-tombi-toml-version" = "v1.1.0")),
)]
pub(crate) struct Config {
    /// Default settings for `prek update` in this project.
    #[serde(alias = "auto_update")]
    pub update: Option<UpdateOptions>,
    /// Configuration-local aliases for numeric hook priorities.
    #[serde(default)]
    pub priorities: BTreeMap<PriorityAlias, u32>,
    pub repos: Vec<Repo>,
    /// A list of `--hook-types` which will be used by default when running `prek install`.
    /// Default is `[pre-commit]`.
    pub default_install_hook_types: Option<Vec<HookType>>,
    /// A mapping from language to the default `language_version`.
    pub default_language_version: Option<FxHashMap<Language, LanguageVersion>>,
    /// A configuration-wide default for the stages property of hooks.
    /// Default to all stages.
    pub default_stages: Option<Stages>,
    /// Default runtime environment variables for hooks.
    pub default_env: Option<FxHashMap<String, String>>,
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

impl Config {
    fn validate_priorities(&self) -> std::result::Result<(), Error> {
        macro_rules! validate_hooks {
            ($hooks:expr) => {
                for hook in $hooks {
                    if let Some(priority) = &hook.priority {
                        priority.resolve(&self.priorities, &hook.id)?;
                    }
                }
            };
        }

        for repo in &self.repos {
            match repo {
                Repo::Remote(repo) => validate_hooks!(&repo.hooks),
                Repo::Local(repo) => validate_hooks!(&repo.hooks),
                Repo::Meta(repo) => validate_hooks!(&repo.hooks),
                Repo::Builtin(repo) => validate_hooks!(&repo.hooks),
            }
        }
        Ok(())
    }

    /// Resolve local relative repository sources from the config file, not the process cwd.
    fn resolve_relative_repo_sources(&mut self, config_path: &Path) -> Result<(), Error> {
        let config_dir = config_path
            .parent()
            .expect("config file must have a parent");
        for repo in &mut self.repos {
            let Repo::Remote(remote) = repo else {
                continue;
            };

            let configured_repo = remote.repo();
            let repo_path = Path::new(configured_repo);
            if configured_repo.starts_with("http://")
                || configured_repo.starts_with("https://")
                || !repo_path.is_relative()
            {
                continue;
            }

            let resolved = config_dir.join(repo_path);
            if resolved.is_dir() {
                remote.set_resolved_source(
                    dunce::canonicalize(resolved)?
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }

        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("Failed to parse `{0}`")]
    Yaml(String, #[source] Box<serde_saphyr::Error>),

    #[error("Failed to parse `{0}`")]
    Toml(String, #[source] Box<toml::de::Error>),

    #[error("Priority alias `{alias}` referenced by hook `{hook}` is not declared in `priorities`")]
    UnknownPriorityAlias { hook: String, alias: PriorityAlias },
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
        // Silently ignore extension keys starting with 'x-' (used for YAML anchors and custom metadata).
        if key.starts_with("x-") {
            continue;
        }
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

        if !repo_unused_keys.is_empty() {
            let repo_prefix = format!("repos[{repo_idx}]");
            push_unused_paths(
                &mut paths,
                &repo_prefix,
                repo_unused_keys.keys().map(String::as_str),
            );
        }
        for (hook_idx, options) in hooks_options.enumerate() {
            if options._unused_keys.is_empty() {
                continue;
            }
            let hook_prefix = format!("repos[{repo_idx}].hooks[{hook_idx}]");
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

    let mut config: Config = match path.extension() {
        Some(ext) if ext.eq_ignore_ascii_case("toml") => toml::from_str(&content)
            .map_err(|e| Error::Toml(path.user_display().to_string(), Box::new(e)))?,
        _ => serde_saphyr::from_str(&content)
            .map_err(|e| Error::Yaml(path.user_display().to_string(), Box::new(e)))?,
    };
    config.validate_priorities()?;
    config.resolve_relative_repo_sources(path)?;

    Ok(config)
}

/// Read the configuration file from the given path, and warn about certain issues.
pub(crate) fn read_config(path: &Path) -> Result<Config, Error> {
    let config = load_config(path)?;

    let unused_paths = collect_unused_paths(&config);
    warn_unused_paths(path, &unused_paths);

    // Check for mutable revs and warn the user.
    let mutable_revs = config
        .repos
        .iter()
        .filter_map(|repo| match repo {
            // A rev is considered mutable if it doesn't contain a '.' (like a version)
            // and is not a hexadecimal string (like a commit SHA).
            Repo::Remote(repo) if !repo.rev.contains('.') && !looks_like_sha(&repo.rev) => {
                Some(repo)
            }
            _ => None,
        })
        .map(|repo| format!("{}: {}", repo.repo().cyan(), repo.rev.yellow()))
        .join("\n");
    if !mutable_revs.is_empty() {
        warn_user!(
            "{}",
            indoc::formatdoc! { r"
            The following repositories use mutable `rev` values (branches or moving tags):
            {}
            `prek` does not automatically detect changes to these references after the first install.
            Use a tag or commit SHA for each `rev`, or run `prek update` to select the latest eligible tag.
            See https://prek.j178.dev/reference/configuration/#rev for details.",
            mutable_revs
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

/// Check if a string looks like a git SHA-1.
pub(crate) fn looks_like_sha(s: &str) -> bool {
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
            .unwrap_or("Upgrade `prek` and try again.".to_string());

        return Err(serde::de::Error::custom(format!(
            "`prek` version `{version}` or newer is required, but version `{cur_version}` is installed. {hint}",
        )));
    }

    Ok(Some(s))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _;

    /// Filter to replace dynamic version in snapshots
    const VERSION_FILTER: (&str, &str) = (
        r"but version `\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?` is installed",
        "but version `[CURRENT_VERSION]` is installed",
    );

    #[test]
    fn config_default_stages_deserialize_empty_as_empty() {
        let parsed: Config =
            serde_saphyr::from_str("repos: []\ndefault_stages: []\n").expect("config should parse");

        assert_eq!(parsed.default_stages, Some(Stages::from([])));
    }

    #[test]
    fn config_default_stages_omitted_keeps_none() {
        let parsed: Config = serde_saphyr::from_str("repos: []\n").expect("config should parse");

        assert_eq!(parsed.default_stages, None);
    }

    #[test]
    fn hook_groups_reject_invalid_names_during_deserialize() {
        for (groups, expected) in [
            (r#"[""]"#, "group name `` cannot be empty"),
            (
                r#"["ci slow"]"#,
                "group name `ci slow` cannot contain whitespace",
            ),
            (
                r#"["ci\tslow"]"#,
                "group name `ci\tslow` cannot contain whitespace",
            ),
        ] {
            let yaml = indoc::formatdoc! {r"
                repos:
                  - repo: local
                    hooks:
                      - id: lint
                        name: Lint
                        language: system
                        entry: echo lint
                        groups: {groups}
            "};
            let err = serde_saphyr::from_str::<Config>(&yaml).unwrap_err();
            let msg = err.to_string();
            assert!(msg.contains(expected), "expected `{expected}` in `{msg}`");
        }
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
        insta::assert_snapshot!(err, @r#"
        error: line 2 column 5: `rev` is not allowed for local repos
         --> <input>:2:5
          |
        1 | repos:
        2 |   - repo: local
          |     ^ `rev` is not allowed for local repos
        3 |     rev: v1.0.0
        4 |     hooks:
          |
        "#);

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
        insta::assert_snapshot!(err, @r#"
        error: line 3 column 5: missing field `rev`
         --> <input>:3:5
          |
        1 | repos:
        2 |   - repo: https://github.com/crate-ci/typos
        3 |     hooks:
          |     ^ missing field `rev`
        4 |       - id: typos
          |
        "#);

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
        insta::assert_snapshot!(err, @r#"
        error: line 5 column 9: missing field `name`
         --> <input>:5:9
          |
        3 |     repo: local
        4 |     hooks:
        5 |       - id: typos
          |         ^ missing field `name`
        "#);

        let yaml = indoc::indoc! {r"
            repos:
              - rev: v1.0.0
                repo: meta
                hooks:
                  - id: typos
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @r#"
        error: line 5 column 9: unknown meta hook id `typos`
         --> <input>:5:9
          |
        3 |     repo: meta
        4 |     hooks:
        5 |       - id: typos
          |         ^ unknown meta hook id `typos`
        "#);

        let yaml = indoc::indoc! {r"
            repos:
              - rev: v1.0.0
                repo: builtin
                hooks:
                  - id: typos
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @r#"
        error: line 5 column 9: unknown builtin hook id `typos`
         --> <input>:5:9
          |
        3 |     repo: builtin
        4 |     hooks:
        5 |       - id: typos
          |         ^ unknown builtin hook id `typos`
        "#);
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
        error: line 6 column 9: missing field `id`
         --> <input>:6:9
          |
        4 |     hooks:
        5 |       - name: typos
        6 |         alias: typo
          |         ^ missing field `id`
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
        insta::assert_snapshot!(err, @r#"
        error: line 4 column 9: missing field `language`
         --> <input>:4:9
          |
        2 |   - repo: local
        3 |     hooks:
        4 |       - id: cargo-fmt
          |         ^ missing field `language`
        5 |         name: cargo fmt
        6 |         entry: cargo fmt
          |
        "#);

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
        error: line 6 column 9: missing field `id`
         --> <input>:6:9
          |
        4 |     hooks:
        5 |       - name: typos
        6 |         alias: typo
          |         ^ missing field `id`
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
        error: line 4 column 9: unknown meta hook id `hello`
         --> <input>:4:9
          |
        2 |   - repo: meta
        3 |     hooks:
        4 |       - id: hello
          |         ^ unknown meta hook id `hello`
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
        error: line 4 column 9: language must be `system` for meta hooks
         --> <input>:4:9
          |
        2 |   - repo: meta
        3 |     hooks:
        4 |       - id: check-hooks-apply
          |         ^ language must be `system` for meta hooks
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
        error: line 4 column 9: `entry` is not allowed for meta hooks
         --> <input>:4:9
          |
        2 |   - repo: meta
        3 |     hooks:
        4 |       - id: check-hooks-apply
          |         ^ `entry` is not allowed for meta hooks
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
    fn language_version_options_toml() -> Result<()> {
        let config: Config = toml::from_str(indoc::indoc! {r#"
            [[repos]]
            repo = "local"

            [[repos.hooks]]
            id = "hook"
            name = "hook"
            entry = "python -m hook"
            language = "python"
            language_version = { request = ">=3.12, <3.13", preference = "only-managed" }
        "#})
        .expect("config should parse");
        let Some(Repo::Local(repo)) = config.repos.first() else {
            anyhow::bail!("expected local repo");
        };
        let Some(language_version) = repo
            .hooks
            .first()
            .and_then(|hook| hook.options.language_version.as_ref())
        else {
            anyhow::bail!("expected language version");
        };

        assert_eq!(language_version.request(), Some(">=3.12, <3.13"));
        assert_eq!(
            language_version.preference(),
            ToolchainPreference::OnlyManaged
        );
        assert!(language_version.allows_download());
        Ok(())
    }

    #[test]
    fn parse_update_options() {
        let yaml = indoc::indoc! {r#"
            update:
              cooldown_days: 7
              freeze: true
              include_tags: "v*"
              exclude_tags: ["*-rc*"]
              repos:
                "https://example.com/repo":
                  include_tags: "v1.*"
                  exclude_tags: [nightly, "*-dev*"]
            repos: []
        "#};
        let result = serde_saphyr::from_str::<Config>(yaml).unwrap();
        let options = result.update.unwrap();

        insta::assert_debug_snapshot!(options, @r###"
        UpdateOptions {
            cooldown_days: Some(
                7,
            ),
            freeze: Some(
                true,
            ),
            include_tags: Some(
                One(
                    "v*",
                ),
            ),
            exclude_tags: Some(
                Many(
                    [
                        "*-rc*",
                    ],
                ),
            ),
            repos: {
                "https://example.com/repo": RepoTagFilterOptions {
                    include_tags: Some(
                        One(
                            "v1.*",
                        ),
                    ),
                    exclude_tags: Some(
                        Many(
                            [
                                "nightly",
                                "*-dev*",
                            ],
                        ),
                    ),
                },
            },
        }
        "###);
    }

    #[test]
    fn parse_update_options_rejects_invalid_glob() {
        let yaml = indoc::indoc! {r#"
            update:
              include_tags: "["
            repos: []
        "#};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();

        insta::assert_snapshot!(err, @r#"
        error: line 2 column 17: error parsing glob '[': unclosed character class; missing ']'
         --> <input>:2:17
          |
        1 | update:
        2 |   include_tags: "["
          |                 ^ error parsing glob '[': unclosed character class; missing ']'
        3 | repos: []
          |
        "#);
    }

    #[test]
    fn parse_legacy_update_key_alias() {
        let yaml = indoc::indoc! {r"
            auto_update:
              cooldown_days: 7
            repos: []
        "};
        let result = serde_saphyr::from_str::<Config>(yaml).unwrap();

        assert_eq!(
            result.update.and_then(|options| options.cooldown_days),
            Some(7)
        );
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
            error: line 8 column 23: `prek` version `10.0.0` or newer is required, but version `[CURRENT_VERSION]` is installed. Upgrade `prek` and try again.
             --> <input>:8:23
              |
            6 |         entry: echo test
            7 |         language: system
            8 | minimum_prek_version: '10.0.0'
              |                       ^ `prek` version `10.0.0` or newer is required, but version `[CURRENT_VERSION]` is installed. Upgrade `prek` and try again.
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
            insta::assert_snapshot!(err, @r#"
            error: line 5 column 25: `prek` version `10.0.0` or newer is required, but version `[CURRENT_VERSION]` is installed. Upgrade `prek` and try again.
             --> <input>:5:25
              |
            3 |   entry: echo test
            4 |   language: system
            5 |   minimum_prek_version: '10.0.0'
              |                         ^ `prek` version `10.0.0` or newer is required, but version `[CURRENT_VERSION]` is installed. Upgrade `prek` and try again.
            "#);
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
        insta::assert_snapshot!(err, @r##"
        error: line 8 column 16: Type tag `pythoon` is not recognized. Check for typos or upgrade prek to get new tags.
         --> <input>:8:16
          |
        6 |         entry: echo
        7 |         language: system
        8 |         types: [pythoon] # Deliberate typo
          |                ^ Type tag `pythoon` is not recognized. Check for typos or upgrade prek to get new tags.
        "##);

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
        insta::assert_snapshot!(err, @r#"
        error: line 8 column 19: Type tag `invalidtag` is not recognized. Check for typos or upgrade prek to get new tags.
         --> <input>:8:19
          |
        6 |         entry: echo
        7 |         language: system
        8 |         types_or: [invalidtag]
          |                   ^ Type tag `invalidtag` is not recognized. Check for typos or upgrade prek to get new tags.
        "#);

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
        insta::assert_snapshot!(err, @r#"
        error: line 8 column 24: Type tag `not-a-real-tag` is not recognized. Check for typos or upgrade prek to get new tags.
         --> <input>:8:24
          |
        6 |         entry: echo
        7 |         language: system
        8 |         exclude_types: [not-a-real-tag]
          |                        ^ Type tag `not-a-real-tag` is not recognized. Check for typos or upgrade prek to get new tags.
        "#);
    }

    #[test]
    fn hook_option_errors_point_to_the_value() {
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: lint
                    name: Lint
                    entry: echo lint
                    language: system
                    always_run: not-a-bool
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @r#"
        error: line 8 column 21: invalid boolean
         --> <input>:8:21
          |
        6 |         entry: echo lint
        7 |         language: system
        8 |         always_run: not-a-bool
          |                     ^ invalid boolean
        "#);

        let yaml = indoc::indoc! {r"
            repos:
              - hooks:
                  - id: lint
                    name: Lint
                    entry: echo lint
                    language: system
                    always_run: not-a-bool
                repo: local
        "};
        let err = serde_saphyr::from_str::<Config>(yaml).unwrap_err();
        insta::assert_snapshot!(err, @r#"
        error: line 7 column 21: invalid boolean
         --> <input>:7:21
          |
        5 |         entry: echo lint
        6 |         language: system
        7 |         always_run: not-a-bool
          |                     ^ invalid boolean
        8 |     repo: local
          |
        "#);

        let toml = indoc::indoc! {r#"
            [[repos]]
            repo = "local"

            [[repos.hooks]]
            id = "lint"
            name = "Lint"
            entry = "echo lint"
            language = "system"
            always_run = "not-a-bool"
        "#};
        let err = toml::from_str::<Config>(toml).unwrap_err();
        insta::assert_snapshot!(err, @r#"
        TOML parse error at line 9, column 14
          |
        9 | always_run = "not-a-bool"
          |              ^^^^^^^^^^^^
        invalid type: string "not-a-bool", expected a boolean
        "#);
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

    #[test]
    fn pass_filenames_zero_is_rejected() {
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: invalid-pass-filenames-zero
                    name: invalid pass_filenames zero
                    entry: echo
                    language: system
                    pass_filenames: 0
        "};
        let result = serde_saphyr::from_str::<Config>(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn pass_filenames_negative_is_rejected() {
        let yaml = indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: invalid-pass-filenames-negative
                    name: invalid pass_filenames negative
                    entry: echo
                    language: system
                    pass_filenames: -1
        "};
        let result = serde_saphyr::from_str::<Config>(yaml);
        assert!(result.is_err());
    }

    #[test]
    fn pass_filenames_string_is_rejected() {
        let yaml = indoc::indoc! {r#"
            repos:
              - repo: local
                hooks:
                  - id: invalid-pass-filenames-string
                    name: invalid pass_filenames string
                    entry: echo
                    language: system
                    pass_filenames: "foo"
        "#};
        let result = serde_saphyr::from_str::<Config>(yaml);
        assert!(result.is_err());
    }
}
