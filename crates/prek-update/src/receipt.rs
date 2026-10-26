use std::env;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use semver::Version;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct Receipt {
    install_prefix: PathBuf,
    pub(crate) source: ReleaseSource,
    pub(crate) version: Version,
    provider: Provider,
    // cargo-dist receipts before 0.23.0 did not record this preference.
    #[serde(default = "default_modify_path")]
    pub(crate) modify_path: bool,
}

fn default_modify_path() -> bool {
    true
}

#[derive(Deserialize)]
struct Provider {
    source: String,
    version: Version,
}

#[derive(Deserialize)]
pub(crate) struct ReleaseSource {
    pub(crate) release_type: String,
    pub(crate) owner: String,
    pub(crate) name: String,
    pub(crate) app_name: String,
}

impl Receipt {
    pub(crate) fn load() -> Result<Self> {
        let path = receipt_directories()?
            .into_iter()
            .map(|directory| directory.join("prek-receipt.json"))
            .find(|path| path.exists())
            .context("No standalone install receipt found for prek")?;
        let content = fs_err::read_to_string(&path)?;
        serde_json::from_str(&content)
            .with_context(|| format!("Failed to load install receipt at `{}`", path.display()))
    }

    pub(crate) fn install_prefix_root(&self) -> Result<&Path> {
        // cargo-dist 0.10.0 through early 0.15.0 prereleases incorrectly included `bin`
        // in receipts for prefix-style installations such as CARGO_HOME.
        if self.provider.source == "cargo-dist"
            && self.provider.version >= "0.10.0-prerelease.1".parse::<Version>()?
            && self.provider.version < "0.15.0-prerelease.8".parse::<Version>()?
            && self
                .install_prefix
                .file_name()
                .is_some_and(|name| name == "bin")
        {
            if let Some(parent) = self.install_prefix.parent() {
                return Ok(parent);
            }
        }
        Ok(&self.install_prefix)
    }

    pub(crate) fn matches_executable(&self, executable: &Path) -> Result<bool> {
        let executable = fs_err::canonicalize(executable)?;
        let mut executable_root = executable.parent().unwrap_or(&executable);
        let root = self.install_prefix_root()?;
        let root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());

        // A prefix installation places binaries in `bin`, but a flat installation may
        // itself be named `bin`. Only strip the executable's parent in the first case.
        if executable_root
            .file_name()
            .is_some_and(|name| name == "bin")
            && root.file_name().is_none_or(|name| name != "bin")
        {
            if let Some(parent) = executable_root.parent() {
                executable_root = parent;
            }
        }
        Ok(executable_root == root)
    }
}

fn receipt_directories() -> Result<Vec<PathBuf>> {
    // Retain axoupdater's overrides for existing standalone installations.
    if EnvVars.var("AXOUPDATER_CONFIG_WORKING_DIR").is_ok() {
        return Ok(vec![env::current_dir()?]);
    }
    if let Ok(path) = EnvVars.var("AXOUPDATER_CONFIG_PATH") {
        return Ok(vec![PathBuf::from(path)]);
    }

    let mut directories = Vec::new();
    if let Ok(path) = EnvVars.var("XDG_CONFIG_HOME") {
        let path = PathBuf::from(path).join("prek");
        if path.exists() {
            directories.push(path);
        }
    }
    let home = if cfg!(windows) {
        EnvVars
            .var_os("LOCALAPPDATA")
            .map(|path| PathBuf::from(path).join("prek"))
    } else {
        EnvVars
            .var_os(EnvVars::HOME)
            .map(PathBuf::from)
            .or_else(env::home_dir)
            .map(|path| path.join(".config/prek"))
    };
    if let Some(home) = home {
        directories.push(home);
    }
    Ok(directories)
}
