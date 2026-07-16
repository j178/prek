use std::collections::BTreeMap;
use std::io::ErrorKind;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use etcetera::BaseStrategy;
use globset::Glob;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use serde::Deserialize;

use crate::config::{StringOrList, UpdateOptions as ProjectUpdateOptions};

fn user_config_path() -> Option<PathBuf> {
    if let Some(path) = EnvVars.var_os(EnvVars::PREK_INTERNAL__USER_CONFIG_PATH) {
        return Some(PathBuf::from(path));
    }

    etcetera::choose_base_strategy()
        .ok()
        .map(|strategy| strategy.config_dir().join("prek").join("prek.toml"))
}

/// Options loaded from a user-level `prek.toml` file.
#[derive(Debug, Clone)]
pub(crate) struct FilesystemOptions(Options);

impl FilesystemOptions {
    /// Load user-level options from the platform config directory.
    pub(crate) fn user() -> Result<Option<Self>> {
        let Some(path) = user_config_path() else {
            tracing::trace!(
                "Skipping global config lookup because no platform config directory was found"
            );
            return Ok(None);
        };

        tracing::trace!(path = %path.display(), "Searching for global config");
        Self::from_file(&path)
    }

    fn from_file(path: &Path) -> Result<Option<Self>> {
        let content = match fs_err::read_to_string(path) {
            Ok(content) => {
                tracing::debug!(path = %path.display(), "Read global config");
                content
            }
            Err(err)
                if matches!(
                    err.kind(),
                    ErrorKind::NotFound | ErrorKind::NotADirectory | ErrorKind::PermissionDenied
                ) =>
            {
                tracing::trace!(
                    path = %path.display(),
                    "Global config not found or inaccessible, skipping"
                );
                return Ok(None);
            }
            Err(err) => {
                return Err(err)
                    .with_context(|| format!("Failed to read global config `{}`", path.display()));
            }
        };

        toml::from_str(&content)
            .map(Self)
            .map(Some)
            .with_context(|| format!("Failed to parse global config `{}`", path.display()))
    }
}

impl Deref for FilesystemOptions {
    type Target = Options;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

/// Options as represented in the global `prek.toml` file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
pub(crate) struct Options {
    #[serde(alias = "auto_update")]
    update: Option<GlobalUpdateOptions>,
}

/// Default update options represented in the global `prek.toml` file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
struct GlobalUpdateOptions {
    cooldown_days: Option<u8>,
    freeze: Option<bool>,
    include_tags: Option<StringOrList>,
    exclude_tags: Option<StringOrList>,
}

/// Tag filters supplied on the command line.
#[derive(Debug, Clone, Default)]
pub(crate) struct CliTagFilterOptions {
    pub(crate) include: Vec<Glob>,
    pub(crate) exclude: Vec<Glob>,
    pub(crate) repo_include: BTreeMap<String, Vec<Glob>>,
    pub(crate) repo_exclude: BTreeMap<String, Vec<Glob>>,
}

/// Effective tag filters for one configured repository.
#[derive(Debug, Clone, Default, Eq, Hash, PartialEq)]
pub(crate) struct TagFilterOptions {
    pub(crate) include: Vec<Glob>,
    pub(crate) exclude: Vec<Glob>,
}

impl TagFilterOptions {
    fn resolve(
        repo: &str,
        cli: &CliTagFilterOptions,
        project: Option<&ProjectUpdateOptions>,
        filesystem: Option<&GlobalUpdateOptions>,
    ) -> Self {
        fn resolve_config_filter(
            repo: Option<&StringOrList>,
            project: Option<&StringOrList>,
            filesystem: Option<&StringOrList>,
        ) -> Vec<Glob> {
            repo.or(project)
                .or(filesystem)
                .map(StringOrList::as_slice)
                .unwrap_or_default()
                .to_vec()
        }

        let repo_options = project.and_then(|options| options.repos.get(repo));
        let mut include = resolve_config_filter(
            repo_options.and_then(|options| options.include_tags.as_ref()),
            project.and_then(|options| options.include_tags.as_ref()),
            filesystem.and_then(|options| options.include_tags.as_ref()),
        );
        let mut exclude = resolve_config_filter(
            repo_options.and_then(|options| options.exclude_tags.as_ref()),
            project.and_then(|options| options.exclude_tags.as_ref()),
            filesystem.and_then(|options| options.exclude_tags.as_ref()),
        );

        if !cli.include.is_empty() {
            include.clone_from(&cli.include);
        }
        if !cli.exclude.is_empty() {
            exclude.clone_from(&cli.exclude);
        }
        if let Some(repo_include) = cli.repo_include.get(repo) {
            include.clone_from(repo_include);
        }
        if let Some(repo_exclude) = cli.repo_exclude.get(repo) {
            exclude.extend(repo_exclude.iter().cloned());
        }

        Self { include, exclude }
    }
}

/// Resolved settings for the `update` command.
#[derive(Debug, Clone)]
pub(crate) struct UpdateSettings {
    pub(crate) cooldown_days: u8,
    pub(crate) freeze: bool,
    pub(crate) tag_filters: TagFilterOptions,
}

impl UpdateSettings {
    pub(crate) fn resolve(
        cli_freeze: bool,
        cli_cooldown_days: Option<u8>,
        repo: &str,
        cli_tag_filters: &CliTagFilterOptions,
        project: Option<&ProjectUpdateOptions>,
        filesystem: Option<&FilesystemOptions>,
    ) -> Self {
        let filesystem_update = filesystem.and_then(|fs| fs.update.as_ref());
        Self {
            cooldown_days: cli_cooldown_days
                .or_else(|| project.and_then(|options| options.cooldown_days))
                .or_else(|| filesystem_update.and_then(|options| options.cooldown_days))
                .unwrap_or_default(),
            freeze: cli_freeze
                || project
                    .and_then(|options| options.freeze)
                    .or_else(|| filesystem_update.and_then(|options| options.freeze))
                    .unwrap_or_default(),
            tag_filters: TagFilterOptions::resolve(
                repo,
                cli_tag_filters,
                project,
                filesystem_update,
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::{CliTagFilterOptions, FilesystemOptions, Options, UpdateSettings};
    use globset::Glob;

    use crate::config::UpdateOptions as ProjectUpdateOptions;

    fn glob_pattern(pattern: &str) -> Glob {
        pattern.parse().unwrap()
    }

    fn pattern_strings(patterns: &[Glob]) -> Vec<&str> {
        patterns.iter().map(Glob::glob).collect()
    }

    #[test]
    fn options_deserializes_update_settings() {
        let options: Options = toml::from_str(
            r#"
            [update]
            cooldown_days = 7
            freeze = true
            include_tags = "v*"
            exclude_tags = ["*-rc*"]
            "#,
        )
        .unwrap();
        let options = options.update.unwrap();

        assert_eq!(options.cooldown_days, Some(7));
        assert_eq!(options.freeze, Some(true));
        assert_eq!(
            pattern_strings(options.include_tags.unwrap().as_slice()),
            ["v*"]
        );
        assert_eq!(
            pattern_strings(options.exclude_tags.unwrap().as_slice()),
            ["*-rc*"]
        );
    }

    #[test]
    fn options_rejects_invalid_update_glob() {
        let err = toml::from_str::<Options>(
            r#"
            [update]
            exclude_tags = ["v*", "["]
            "#,
        )
        .unwrap_err();

        insta::assert_snapshot!(err, @r#"
        TOML parse error at line 3, column 28
          |
        3 |             exclude_tags = ["v*", "["]
          |                            ^^^^^^^^^^^
        error parsing glob '[': unclosed character class; missing ']'
        "#);
    }

    #[test]
    fn options_deserializes_legacy_update_key_alias() {
        let options: Options = toml::from_str(
            r"
            [auto_update]
            cooldown_days = 7
            ",
        )
        .unwrap();

        assert_eq!(
            options.update.and_then(|options| options.cooldown_days),
            Some(7)
        );
    }

    #[test]
    fn update_settings_uses_global_freeze() {
        let filesystem = FilesystemOptions(
            toml::from_str(
                r"
                [update]
                freeze = true
                ",
            )
            .unwrap(),
        );

        let settings = UpdateSettings::resolve(
            false,
            None,
            "https://example.com/repo",
            &CliTagFilterOptions::default(),
            None,
            Some(&filesystem),
        );

        assert!(settings.freeze);
    }

    #[test]
    fn update_settings_project_freeze_overrides_global() {
        let filesystem = FilesystemOptions(
            toml::from_str(
                r"
                [update]
                freeze = true
                ",
            )
            .unwrap(),
        );

        let project = ProjectUpdateOptions {
            cooldown_days: None,
            freeze: Some(false),
            ..ProjectUpdateOptions::default()
        };
        let settings = UpdateSettings::resolve(
            false,
            None,
            "https://example.com/repo",
            &CliTagFilterOptions::default(),
            Some(&project),
            Some(&filesystem),
        );

        assert!(!settings.freeze);
    }

    #[test]
    fn update_settings_cli_freeze_overrides_project() {
        let project = ProjectUpdateOptions {
            cooldown_days: None,
            freeze: Some(false),
            ..ProjectUpdateOptions::default()
        };
        let settings = UpdateSettings::resolve(
            true,
            None,
            "https://example.com/repo",
            &CliTagFilterOptions::default(),
            Some(&project),
            None,
        );

        assert!(settings.freeze);
    }

    #[test]
    fn update_settings_resolves_tag_filter_precedence_per_option() {
        let filesystem = FilesystemOptions(
            toml::from_str(
                r#"
                [update]
                include_tags = "global-include"
                exclude_tags = ["global-exclude"]
                "#,
            )
            .unwrap(),
        );
        let project: ProjectUpdateOptions = toml::from_str(
            r#"
            include_tags = "project-include"
            exclude_tags = ["project-exclude"]

            [repos."https://example.com/repo"]
            include_tags = []
            exclude_tags = "repo-exclude"
            "#,
        )
        .unwrap();
        let cli = CliTagFilterOptions {
            include: vec![glob_pattern("cli-include")],
            repo_exclude: BTreeMap::from([(
                "https://example.com/repo".to_string(),
                vec![glob_pattern("cli-repo-exclude")],
            )]),
            ..CliTagFilterOptions::default()
        };

        let settings = UpdateSettings::resolve(
            false,
            None,
            "https://example.com/repo",
            &cli,
            Some(&project),
            Some(&filesystem),
        );

        assert_eq!(
            pattern_strings(&settings.tag_filters.include),
            ["cli-include"]
        );
        assert_eq!(
            pattern_strings(&settings.tag_filters.exclude),
            ["repo-exclude", "cli-repo-exclude"]
        );
    }

    #[test]
    fn update_settings_empty_repo_filter_clears_project_default() {
        let project: ProjectUpdateOptions = toml::from_str(
            r#"
            include_tags = "v*"

            [repos."https://example.com/repo"]
            include_tags = []
            "#,
        )
        .unwrap();

        let settings = UpdateSettings::resolve(
            false,
            None,
            "https://example.com/repo",
            &CliTagFilterOptions::default(),
            Some(&project),
            None,
        );

        assert!(settings.tag_filters.include.is_empty());
    }
}
