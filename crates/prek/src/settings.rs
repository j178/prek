use std::io::ErrorKind;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use etcetera::BaseStrategy;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use serde::Deserialize;

use crate::config::UpdateOptions as ProjectUpdateOptions;

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
    update: Option<UpdateOptions>,
}

/// Options for the `update` command.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
struct UpdateOptions {
    cooldown_days: Option<u8>,
    freeze: Option<bool>,
}

/// Resolved settings for the `update` command.
#[derive(Debug, Clone, Copy)]
pub(crate) struct UpdateSettings {
    pub(crate) cooldown_days: u8,
    pub(crate) freeze: bool,
}

impl UpdateSettings {
    pub(crate) fn resolve(
        cli_freeze: bool,
        cli_cooldown_days: Option<u8>,
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{FilesystemOptions, Options, UpdateSettings};
    use crate::config::UpdateOptions as ProjectUpdateOptions;

    #[test]
    fn options_deserializes_update_settings() {
        let options: Options = toml::from_str(
            r"
            [update]
            cooldown_days = 7
            freeze = true
            ",
        )
        .unwrap();

        assert_eq!(
            options
                .update
                .map(|options| (options.cooldown_days, options.freeze)),
            Some((Some(7), Some(true)))
        );
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

        let settings = UpdateSettings::resolve(false, None, None, Some(&filesystem));

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
        };
        let settings = UpdateSettings::resolve(false, None, Some(&project), Some(&filesystem));

        assert!(!settings.freeze);
    }

    #[test]
    fn update_settings_cli_freeze_overrides_project() {
        let project = ProjectUpdateOptions {
            cooldown_days: None,
            freeze: Some(false),
        };
        let settings = UpdateSettings::resolve(true, None, Some(&project), None);

        assert!(settings.freeze);
    }
}
