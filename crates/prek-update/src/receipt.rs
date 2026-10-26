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

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn receipt(prefix: &Path, provider: &str, provider_version: &str) -> Result<Receipt> {
        Ok(serde_json::from_value(json!({
            "install_prefix": prefix,
            "binaries": ["prek"],
            "version": "1.2.3",
            "provider": {"source": provider, "version": provider_version},
            "source": {"release_type": "github", "owner": "j178", "name": "prek", "app_name": "prek"},
        }))?)
    }

    /// Only receipts from the affected cargo-dist releases strip the erroneous trailing bin.
    #[test]
    fn legacy_prefix_boundaries() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let bin = directory.path().join("bin");
        for (provider, version, strip_bin) in [
            ("cargo-dist", "0.9.9", false),
            ("cargo-dist", "0.10.0-prerelease.0", false),
            ("cargo-dist", "0.10.0-prerelease.1", true),
            ("cargo-dist", "0.14.0", true),
            ("cargo-dist", "0.15.0-prerelease.7", true),
            ("cargo-dist", "0.15.0-prerelease.8", false),
            ("cargo-dist", "0.15.0", false),
            ("another-installer", "0.14.0", false),
        ] {
            let receipt = receipt(&bin, provider, version)?;
            let expected = if strip_bin { directory.path() } else { &bin };
            assert_eq!(
                receipt.install_prefix_root()?,
                expected,
                "{provider} {version}"
            );
        }
        let prefix = directory.path().join("flat");
        assert_eq!(
            receipt(&prefix, "cargo-dist", "0.14.0")?.install_prefix_root()?,
            prefix
        );
        Ok(())
    }

    /// Both flat and prefix layouts must match, including flat directories named bin.
    #[test]
    fn installation_layouts() -> Result<()> {
        let directory = tempfile::tempdir()?;
        let root = directory.path().canonicalize()?;
        let bin = root.join("bin");
        fs_err::create_dir(&bin)?;
        let executable = bin.join("prek");
        fs_err::write(&executable, "executable")?;
        for (prefix, provider_version, matches) in [
            (root.clone(), "0.30.0", true),
            (bin.clone(), "0.30.0", true),
            (bin.clone(), "0.14.0", true),
            (root.join("elsewhere"), "0.30.0", false),
        ] {
            assert_eq!(
                receipt(&prefix, "cargo-dist", provider_version)?
                    .matches_executable(&executable)?,
                matches,
                "{}",
                prefix.display()
            );
        }

        let flat_executable = root.join("prek");
        fs_err::write(&flat_executable, "executable")?;
        assert!(receipt(&root, "cargo-dist", "0.30.0")?.matches_executable(&flat_executable)?);
        #[cfg(unix)]
        {
            let alias = root.join("alias");
            std::os::unix::fs::symlink(&bin, &alias)?;
            assert!(receipt(&alias, "cargo-dist", "0.30.0")?.matches_executable(&executable)?);
        }
        Ok(())
    }

    fn write_receipt(directory: &Path, version: &str) -> Result<()> {
        fs_err::create_dir_all(directory)?;
        fs_err::write(
            directory.join("prek-receipt.json"),
            serde_json::to_vec(&json!({
                "install_prefix": directory,
                "binaries": ["prek"],
                "version": version,
                "provider": {"source": "cargo-dist", "version": "0.14.0"},
                "source": {"release_type": "github", "owner": "j178", "name": "prek", "app_name": "prek"},
            }))?,
        )?;
        Ok(())
    }

    /// Explicit overrides win; XDG falls back only when no receipt exists, not on bad JSON.
    #[test]
    fn discovery_precedence() -> Result<()> {
        if crate::tests::is_child() {
            let expected = EnvVars.var("PREK_UPDATE_TEST_EXPECTED")?;
            let result = Receipt::load();
            if expected == "error" {
                assert!(result.is_err());
            } else {
                let receipt = result?;
                assert_eq!(receipt.version.to_string(), expected);
                // Older cargo-dist receipts omit modify_path and must keep the old default.
                assert!(receipt.modify_path);
            }
            return Ok(());
        }

        for (case, expected) in [
            ("home", "4.0.0"),
            ("xdg", "3.0.0"),
            ("xdg-empty", "4.0.0"),
            ("xdg-missing", "4.0.0"),
            ("explicit", "2.0.0"),
            ("working-directory", "1.0.0"),
            ("explicit-missing", "error"),
            ("xdg-corrupt", "error"),
            ("missing", "error"),
        ] {
            let temporary = tempfile::tempdir()?;
            let root = temporary.path();
            let home = root.join("home");
            let local_app_data = root.join("local-app-data");
            let config = if cfg!(windows) {
                local_app_data.join("prek")
            } else {
                home.join(".config/prek")
            };
            let xdg = root.join("xdg");
            let explicit = root.join("explicit");
            let mut command = crate::tests::child()?;
            command
                .current_dir(root)
                .env("HOME", &home)
                .env("LOCALAPPDATA", &local_app_data)
                .env_remove("XDG_CONFIG_HOME")
                .env_remove("AXOUPDATER_CONFIG_PATH")
                .env_remove("AXOUPDATER_CONFIG_WORKING_DIR")
                .env("PREK_UPDATE_TEST_EXPECTED", expected);
            if case != "missing" {
                write_receipt(&config, "4.0.0")?;
            }
            if !matches!(case, "home" | "missing") {
                command.env("XDG_CONFIG_HOME", &xdg);
                if case == "xdg-empty" {
                    fs_err::create_dir_all(xdg.join("prek"))?;
                } else if case != "xdg-missing" {
                    write_receipt(&xdg.join("prek"), "3.0.0")?;
                }
            }
            if matches!(case, "explicit" | "working-directory" | "explicit-missing") {
                command.env("AXOUPDATER_CONFIG_PATH", &explicit);
                if case != "explicit-missing" {
                    write_receipt(&explicit, "2.0.0")?;
                }
            }
            if case == "working-directory" {
                write_receipt(root, "1.0.0")?;
                command.env("AXOUPDATER_CONFIG_WORKING_DIR", "");
            }
            if case == "xdg-corrupt" {
                fs_err::write(xdg.join("prek/prek-receipt.json"), "invalid JSON")?;
            }
            crate::tests::check_child(&mut command).with_context(|| case.to_owned())?;
        }
        Ok(())
    }
}
