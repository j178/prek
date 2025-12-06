//! Unified configuration settings for prek.
//!
//! Settings are resolved from multiple sources with the following precedence (highest to lowest):
//! 1. CLI flags (merged via `CliOverrides`)
//! 2. `pyproject.toml` `[tool.prek]` section
//! 3. Environment variables (`PREK_*`)
//! 4. Built-in defaults

use std::path::{Path, PathBuf};
use std::sync::{LazyLock, RwLock};

use prek_consts::env_vars::EnvVars;
use serde::Deserialize;

/// Global settings instance, initialized lazily.
///
/// Call `Settings::init_with_cli()` early in main to set the working directory,
/// or it will default to the current directory.
static SETTINGS: LazyLock<RwLock<Settings>> = LazyLock::new(|| RwLock::new(Settings::default()));

/// Check if settings have been initialized
static INITIALIZED: LazyLock<RwLock<bool>> = LazyLock::new(|| RwLock::new(false));

/// Prek configuration settings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[allow(clippy::struct_excessive_bools)]
pub struct Settings {
    /// Hook IDs to skip (equivalent to `PREK_SKIP`).
    pub skip: Vec<String>,
    /// Override the prek data directory (equivalent to `PREK_HOME`).
    pub home: Option<PathBuf>,
    /// Control colored output: "auto", "always", or "never" (equivalent to `PREK_COLOR`).
    pub color: Option<ColorChoice>,
    /// Allow running without a `.pre-commit-config.yaml` (equivalent to `PREK_ALLOW_NO_CONFIG`).
    pub allow_no_config: bool,
    /// Disable parallelism for installs and runs (equivalent to `PREK_NO_CONCURRENCY`).
    pub no_concurrency: bool,
    /// Disable Rust-native built-in hooks (equivalent to `PREK_NO_FAST_PATH`).
    pub no_fast_path: bool,
    /// Control how uv is installed (equivalent to `PREK_UV_SOURCE`).
    pub uv_source: Option<String>,
    /// Use system's trusted store instead of bundled roots (equivalent to `PREK_NATIVE_TLS`).
    pub native_tls: bool,
    /// Container runtime to use: "auto", "docker", or "podman" (equivalent to `PREK_CONTAINER_RUNTIME`).
    pub container_runtime: Option<ContainerRuntime>,
}

/// Settings as parsed from pyproject.toml `[tool.prek]`
#[derive(Debug, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
#[allow(clippy::struct_excessive_bools)]
struct PyProjectSettings {
    skip: Vec<String>,
    home: Option<PathBuf>,
    color: Option<ColorChoice>,
    allow_no_config: bool,
    no_concurrency: bool,
    no_fast_path: bool,
    uv_source: Option<String>,
    native_tls: bool,
    container_runtime: Option<ContainerRuntime>,
}

/// Wrapper to extract `[tool.prek]` from pyproject.toml
#[derive(Debug, Deserialize)]
struct PyProjectToml {
    tool: Option<PyProjectTool>,
}

#[derive(Debug, Deserialize)]
struct PyProjectTool {
    prek: Option<PyProjectSettings>,
}

/// Color output choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ColorChoice {
    /// Enables colored output only when the output is going to a terminal or TTY with support.
    #[default]
    Auto,
    /// Enables colored output regardless of the detected environment.
    Always,
    /// Disables colored output.
    Never,
}

impl std::fmt::Display for ColorChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Always => write!(f, "always"),
            Self::Never => write!(f, "never"),
        }
    }
}

impl From<ColorChoice> for anstream::ColorChoice {
    fn from(value: ColorChoice) -> Self {
        match value {
            ColorChoice::Auto => Self::Auto,
            ColorChoice::Always => Self::Always,
            ColorChoice::Never => Self::Never,
        }
    }
}

impl std::str::FromStr for ColorChoice {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "always" => Ok(Self::Always),
            "never" => Ok(Self::Never),
            _ => Err(format!("invalid color choice: {s}")),
        }
    }
}

/// Container runtime choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "lowercase")]
pub enum ContainerRuntime {
    /// Auto-detect available runtime.
    #[default]
    Auto,
    /// Use Docker.
    Docker,
    /// Use Podman.
    Podman,
}

impl std::str::FromStr for ContainerRuntime {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "docker" => Ok(Self::Docker),
            "podman" => Ok(Self::Podman),
            _ => Err(format!("invalid container runtime: {s}")),
        }
    }
}

impl std::fmt::Display for ContainerRuntime {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Auto => write!(f, "auto"),
            Self::Docker => write!(f, "docker"),
            Self::Podman => write!(f, "podman"),
        }
    }
}

/// CLI overrides that take highest precedence.
///
/// This struct contains only the fields that can be set via CLI flags.
/// Fields are all optional - `None` means "use value from other sources".
#[derive(Copy, Debug, Clone, Default)]
pub struct CliOverrides {
    pub color: Option<ColorChoice>,
}

impl CliOverrides {
    pub fn new() -> Self {
        Self::default()
    }

    #[must_use]
    pub fn color(mut self, color: Option<ColorChoice>) -> Self {
        self.color = color;
        self
    }
}

impl Settings {
    /// Initialize settings with CLI overrides.
    ///
    /// This should be called early in `main()` before any settings are accessed.
    pub fn init_with_cli(working_dir: &Path, cli: CliOverrides) {
        let settings = Self::load(working_dir, cli);
        *SETTINGS.write().expect("settings lock poisoned") = settings;
        *INITIALIZED.write().expect("initialized lock poisoned") = true;
    }

    /// Get the global settings instance.
    ///
    /// If settings haven't been initialized, this will initialize them
    /// using the current directory.
    pub fn get() -> Settings {
        {
            let initialized = *INITIALIZED.read().expect("initialized lock poisoned");
            if !initialized {
                let cwd = std::env::current_dir().unwrap_or_default();
                Self::init_with_cli(&cwd, CliOverrides::default());
            }
        }
        SETTINGS.read().expect("settings lock poisoned").clone()
    }

    /// Load and merge settings from all sources.
    fn load(start_dir: &Path, cli: CliOverrides) -> Self {
        // Start with pyproject.toml settings (or defaults)
        let pyproject = Self::load_pyproject(start_dir).unwrap_or_default();

        // Layer env vars on top, then CLI
        Settings {
            skip: env_list("PREK_SKIP")
                .or_else(|| env_list("SKIP"))
                .unwrap_or(pyproject.skip),
            home: env_path("PREK_HOME").or(pyproject.home),
            color: cli
                .color
                .or_else(|| env_parse::<ColorChoice>("PREK_COLOR"))
                .or(pyproject.color),
            allow_no_config: env_bool("PREK_ALLOW_NO_CONFIG").unwrap_or(pyproject.allow_no_config),
            no_concurrency: env_bool("PREK_NO_CONCURRENCY").unwrap_or(pyproject.no_concurrency),
            no_fast_path: env_bool("PREK_NO_FAST_PATH").unwrap_or(pyproject.no_fast_path),
            uv_source: env_string("PREK_UV_SOURCE").or(pyproject.uv_source),
            native_tls: env_bool("PREK_NATIVE_TLS").unwrap_or(pyproject.native_tls),
            container_runtime: env_parse("PREK_CONTAINER_RUNTIME").or(pyproject.container_runtime),
        }
    }

    /// Walk up the directory tree to find and parse pyproject.toml
    fn load_pyproject(start_dir: &Path) -> Option<PyProjectSettings> {
        let mut dir = Some(start_dir);
        while let Some(d) = dir {
            if let Ok(content) = std::fs::read_to_string(d.join("pyproject.toml")) {
                if let Ok(parsed) = toml::from_str::<PyProjectToml>(&content) {
                    if let Some(settings) = parsed.tool.and_then(|t| t.prek) {
                        return Some(settings);
                    }
                }
            }
            dir = d.parent();
        }
        None
    }

    /// Check if a hook should be skipped.
    pub fn should_skip(&self, hook_id: &str) -> bool {
        self.skip.iter().any(|s| s == hook_id)
    }

    /// Get the resolved color choice.
    pub fn resolved_color(&self) -> ColorChoice {
        self.color.unwrap_or_default()
    }

    /// Get the resolved container runtime.
    pub fn resolved_container_runtime(&self) -> ContainerRuntime {
        self.container_runtime.unwrap_or_default()
    }
}

// Simple env var helpers
fn env_string(key: &str) -> Option<String> {
    EnvVars::var(key).ok()
}

fn env_path(key: &str) -> Option<PathBuf> {
    env_string(key).map(PathBuf::from)
}

fn env_bool(key: &str) -> Option<bool> {
    EnvVars::var_as_bool(key)
}

fn env_parse<T: std::str::FromStr>(key: &str) -> Option<T> {
    env_string(key).and_then(|s| s.parse().ok())
}

fn env_list(key: &str) -> Option<Vec<String>> {
    env_string(key).map(|s| {
        s.split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_default_settings() {
        let settings = Settings::default();
        assert!(settings.skip.is_empty());
        assert!(settings.home.is_none());
        assert!(!settings.no_concurrency);
    }

    #[test]
    fn test_pyproject_loading() {
        let dir = TempDir::new().unwrap();
        let pyproject = dir.path().join("pyproject.toml");

        let mut file = std::fs::File::create(&pyproject).unwrap();
        write!(
            file,
            r#"
[tool.prek]
skip = ["black", "ruff"]
no-concurrency = true
color = "always"
"#
        )
        .unwrap();

        let settings = Settings::load(dir.path(), CliOverrides::default());
        assert_eq!(settings.skip, vec!["black", "ruff"]);
        assert!(settings.no_concurrency);
        assert_eq!(settings.color, Some(ColorChoice::Always));
    }

    #[test]
    fn test_walks_up_directory_tree() {
        let dir = TempDir::new().unwrap();
        let pyproject = dir.path().join("pyproject.toml");

        let mut file = std::fs::File::create(&pyproject).unwrap();
        write!(
            file,
            r#"
[tool.prek]
skip = ["parent-hook"]
"#
        )
        .unwrap();

        std::fs::create_dir_all(dir.path().join("subdir/nested")).unwrap();

        let settings = Settings::load(&dir.path().join("subdir/nested"), CliOverrides::default());
        assert_eq!(settings.skip, vec!["parent-hook"]);
    }

    #[test]
    fn test_cli_overrides_pyproject() {
        let dir = TempDir::new().unwrap();
        let pyproject = dir.path().join("pyproject.toml");

        let mut file = std::fs::File::create(&pyproject).unwrap();
        write!(
            file,
            r#"
[tool.prek]
color = "auto"
"#
        )
        .unwrap();

        let settings = Settings::load(
            dir.path(),
            CliOverrides::new().color(Some(ColorChoice::Always)),
        );
        assert_eq!(settings.color, Some(ColorChoice::Always));
    }
}
