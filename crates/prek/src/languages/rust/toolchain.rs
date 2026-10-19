use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use semver::Version;

use crate::process::Cmd;

/// A Rust compiler with a known version and Cargo in the same directory.
#[derive(Debug)]
pub(crate) struct RustToolchain {
    pub(crate) rustc: PathBuf,
    pub(crate) version: Version,
}

impl RustToolchain {
    pub(crate) async fn inspect_dir(dir: &Path) -> Result<Self> {
        let bin = dir.join("bin");
        let rustc = bin.join("rustc").with_extension(EXE_EXTENSION);
        let version = Self::query_version(&rustc).await?;

        Ok(Self { rustc, version })
    }

    pub(crate) async fn query_version(rustc: &Path) -> Result<Version> {
        let output = Cmd::new(rustc)
            .arg("-vV")
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
            .check(true)
            .output()
            .await
            .with_context(|| format!("Failed to query Rust version from {}", rustc.display()))?;

        let output = str::from_utf8(&output.stdout)?;
        let release = output
            .lines()
            .find_map(|line| line.strip_prefix("release: "))
            .with_context(|| format!("Failed to parse Rust version from output: {output}"))?;

        Version::parse(release).context("Invalid Rust release version")
    }

    pub(crate) fn bin_dir(&self) -> Result<&Path> {
        self.rustc
            .parent()
            .context("Rust compiler has no parent directory")
    }

    pub(crate) fn command(&self, executable: &Path) -> Cmd {
        let mut cmd = Cmd::new(executable);
        cmd.env(EnvVars::RUSTC, &self.rustc)
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0");

        cmd
    }

    pub(crate) fn cargo_command(&self) -> Cmd {
        let cargo = self
            .rustc
            .with_file_name("cargo")
            .with_extension(EXE_EXTENSION);

        self.command(&cargo)
    }
}
