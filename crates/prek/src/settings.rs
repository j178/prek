use std::io::ErrorKind;
use std::ops::Deref;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use etcetera::BaseStrategy;
use prek_consts::env_vars::EnvVars;
use serde::Deserialize;

use crate::cli::AutoUpdateArgs;

fn user_config_path() -> Option<PathBuf> {
    if let Some(path) = EnvVars::var_os(EnvVars::PREK_INTERNAL__USER_CONFIG_PATH) {
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
    auto_update: Option<AutoUpdateOptions>,
}

/// Options for the `auto-update` command.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "snake_case")]
struct AutoUpdateOptions {
    cooldown_days: Option<u8>,
}

/// Resolved settings for the `auto-update` command.
#[derive(Debug, Clone, Copy)]
pub(crate) struct AutoUpdateSettings {
    pub(crate) cooldown_days: u8,
}

impl AutoUpdateSettings {
    pub(crate) fn resolve(args: &AutoUpdateArgs, filesystem: Option<&FilesystemOptions>) -> Self {
        Self {
            cooldown_days: args
                .cooldown_days
                .or_else(|| {
                    filesystem
                        .and_then(|fs| fs.auto_update.as_ref())
                        .and_then(|au| au.cooldown_days)
                })
                .unwrap_or_default(),
        }
    }
}
