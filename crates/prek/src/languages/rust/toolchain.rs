use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use prek_consts::env_vars::EnvVars;
use semver::Version;
use tracing::debug;

use crate::languages::rust::rustup::Rustup;
use crate::process::Cmd;

/// A compiler and Cargo pair with explicit paths and a known compiler version.
#[derive(Debug)]
pub(crate) struct RustToolchain {
    pub(crate) rustc: PathBuf,
    pub(crate) cargo: PathBuf,
    pub(crate) version: Version,
}

impl RustToolchain {
    pub(crate) async fn inspect_dir(dir: &Path) -> Result<Self> {
        let bin = dir.join("bin");
        let rustc = bin.join("rustc").with_extension(EXE_EXTENSION);
        let version = Self::query_version(&rustc).await?;

        Ok(Self {
            rustc,
            cargo: bin.join("cargo").with_extension(EXE_EXTENSION),
            version,
        })
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

    /// Pair a known compiler with a usable Cargo from the same directory or PATH.
    pub(crate) async fn find_cargo(
        rustc: &Path,
        version: &Version,
        cwd: &Path,
        excluded_root: Option<&Path>,
    ) -> Option<Self> {
        // Prefer the compiler's Cargo when it is available, but allow split packages.
        let sibling = rustc.with_file_name("cargo").with_extension(EXE_EXTENSION);
        let paths = which::which_all("cargo").into_iter().flatten();

        for cargo in std::iter::once(sibling).chain(paths) {
            if !cargo.is_file() {
                continue;
            }

            let cargo = match Rustup::resolve_proxy(&cargo, "cargo", cwd).await {
                Ok(cargo) => cargo,
                Err(err) => {
                    debug!("Skipping unusable Cargo: {err:#}");
                    continue;
                }
            };

            if let Some(root) = excluded_root {
                let resolved = dunce::canonicalize(&cargo).unwrap_or_else(|_| cargo.clone());
                if resolved.starts_with(root) {
                    continue;
                }
            }

            let toolchain = Self {
                rustc: rustc.to_path_buf(),
                cargo,
                version: version.clone(),
            };
            if let Err(err) = toolchain.check_cargo().await {
                debug!("Skipping unusable Cargo: {err:#}");
                continue;
            }

            return Some(toolchain);
        }

        None
    }

    pub(crate) fn bin_dirs(&self) -> Result<[&Path; 2]> {
        let rustc_bin = self
            .rustc
            .parent()
            .context("Rust compiler has no parent directory")?;
        let cargo_bin = self
            .cargo
            .parent()
            .context("Cargo has no parent directory")?;

        if rustc_bin == cargo_bin
            || !rustc_bin
                .join("cargo")
                .with_extension(EXE_EXTENSION)
                .is_file()
        {
            return Ok([rustc_bin, cargo_bin]);
        }

        // A fallback Cargo must precede rustc's unusable sibling without hiding rustc itself.
        if cargo_bin
            .join("rustc")
            .with_extension(EXE_EXTENSION)
            .is_file()
        {
            anyhow::bail!("Rust and Cargo directories contain conflicting executables");
        }

        Ok([cargo_bin, rustc_bin])
    }

    pub(crate) fn command(&self, executable: &Path) -> Cmd {
        let mut cmd = Cmd::new(executable);
        cmd.env(EnvVars::RUSTC, &self.rustc)
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0");

        cmd
    }

    pub(crate) fn cargo_command(&self) -> Cmd {
        self.command(&self.cargo)
    }

    pub(crate) async fn check_cargo(&self) -> Result<()> {
        self.bin_dirs()?;

        self.cargo_command()
            .arg("--version")
            .check(true)
            .output()
            .await
            .with_context(|| format!("Failed to query Cargo at {}", self.cargo.display()))?;

        Ok(())
    }
}
