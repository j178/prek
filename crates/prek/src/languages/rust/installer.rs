use std::fmt::Display;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;
use semver::Version;
use tracing::{debug, trace};

use crate::fs::LockedFile;
use crate::languages::rust::RustRequest;
use crate::languages::rust::rustup::{Rustup, ToolchainInfo};
use crate::languages::rust::version::{Channel, RustVersion};
use crate::process::Cmd;

pub(crate) struct RustResult {
    toolchain: PathBuf,
    version: RustVersion,
}

impl Display for RustResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.toolchain.display(), *self.version)?;
        Ok(())
    }
}

impl RustResult {
    pub(crate) fn from_dir(dir: &Path) -> Self {
        Self {
            toolchain: dir.to_path_buf(),
            version: RustVersion::default(),
        }
    }

    pub(crate) fn toolchain(&self) -> &Path {
        &self.toolchain
    }

    pub(crate) fn version(&self) -> &RustVersion {
        &self.version
    }

    pub(crate) fn with_version(mut self, version: RustVersion) -> Self {
        self.version = version;
        self
    }

    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        let rustc = self
            .toolchain
            .join("bin")
            .join("rustc")
            .with_extension(std::env::consts::EXE_EXTENSION);

        let output = Cmd::new(rustc, "rustc --version")
            .arg("--version")
            .env(EnvVars::RUSTUP_AUTO_INSTALL, "0")
            .check(true)
            .output()
            .await?;

        // e.g. "rustc 1.70.0 (90c541806 2023-05-31)"
        let version_str = str::from_utf8(&output.stdout)?;
        let version_str = version_str
            .split_ascii_whitespace()
            .nth(1)
            .with_context(|| format!("Failed to parse Rust version from output: {version_str}"))?;

        let version = Version::parse(version_str)?;
        let version = RustVersion::from_path(&version, &self.toolchain);

        self.version = version;

        Ok(self)
    }
}

pub(crate) struct RustInstaller {
    rustup: Rustup,
}

impl RustInstaller {
    pub(crate) fn new(rustup: Rustup) -> Self {
        Self { rustup }
    }

    pub(crate) async fn install(
        &self,
        request: &RustRequest,
        allows_download: bool,
    ) -> Result<RustResult> {
        let rustup_home = self.rustup.rustup_home();
        fs_err::tokio::create_dir_all(rustup_home).await?;
        let _lock = LockedFile::acquire(rustup_home.join(".lock"), "rustup").await?;

        // Check installed
        if let Ok(rust) = self.find_installed(request).await {
            trace!(%rust, "Found installed rust");
            return Ok(rust);
        }

        // Check system rust
        if let Some(rust) = self.find_system_rust(request).await? {
            trace!(%rust, "Using system rust");
            return Ok(rust);
        }

        if !allows_download {
            anyhow::bail!("No suitable system Rust version found and downloads are disabled");
        }

        // Install new toolchain
        let toolchain = self.resolve_version(request).await?;
        self.download(&toolchain).await
    }

    async fn find_installed(&self, request: &RustRequest) -> Result<RustResult> {
        let mut toolchains: Vec<ToolchainInfo> = self.rustup.list_installed_toolchains().await?;

        sort_toolchains(&mut toolchains);

        toolchains
            .into_iter()
            .find_map(|info| {
                let matches = request.matches(&info.version, Some(&info.path));

                if matches {
                    trace!(name = %info.name, "Found matching installed rust");
                    Some(RustResult::from_dir(&info.path).with_version(info.version))
                } else {
                    trace!(name = %info.name, "Installed rust does not match request");
                    None
                }
            })
            .context("No installed rust version matches the request")
    }

    async fn find_system_rust(&self, rust_request: &RustRequest) -> Result<Option<RustResult>> {
        let mut toolchains: Vec<ToolchainInfo> = self.rustup.list_system_toolchains().await?;

        sort_toolchains(&mut toolchains);

        for info in toolchains {
            let matches = rust_request.matches(&info.version, Some(&info.path));

            if matches {
                trace!(name = %info.name, "Found matching system rust");
                let rust = RustResult::from_dir(&info.path).with_version(info.version);
                return Ok(Some(rust));
            }
            trace!(name = %info.name, "System rust does not match request");
        }

        debug!(
            ?rust_request,
            "No system rust matches the requested version"
        );
        Ok(None)
    }

    async fn resolve_version(&self, req: &RustRequest) -> Result<RustVersion> {
        match req {
            RustRequest::Any => Ok(RustVersion::from_channel(Channel::Stable)),
            RustRequest::Channel(ch) => Ok(RustVersion::from_channel(*ch)),

            RustRequest::Major(_)
            | RustRequest::MajorMinor(_, _)
            | RustRequest::MajorMinorPatch(_, _, _)
            | RustRequest::Range(_, _) => {
                let output = crate::git::git_cmd("list rust tags")?
                    .arg("ls-remote")
                    .arg("--tags")
                    .arg("https://github.com/rust-lang/rust")
                    .output()
                    .await?
                    .stdout;
                let versions: Vec<RustVersion> = str::from_utf8(&output)?
                    .lines()
                    .filter_map(|line| {
                        let tag = line.split('\t').nth(1)?;
                        let tag = tag.strip_prefix("refs/tags/")?;
                        Version::parse(tag)
                            .ok()
                            .map(|v| RustVersion::from_version(&v))
                    })
                    .sorted_unstable_by(|a, b| b.cmp(a))
                    .collect();

                let version = versions
                    .into_iter()
                    .find(|version| req.matches(version, None))
                    .with_context(|| format!("Version `{req}` not found on remote"))?;
                Ok(version)
            }
        }
    }

    async fn download(&self, toolchain: &RustVersion) -> Result<RustResult> {
        let toolchain = toolchain.to_toolchain_name();
        debug!(%toolchain, "Installing Rust toolchain");

        let toolchain_dir = self
            .rustup
            .install_toolchain(&toolchain)
            .await
            .context("Failed to install Rust toolchain")?;

        let rust = RustResult::from_dir(&toolchain_dir).fill_version().await?;
        Ok(rust)
    }
}

fn sort_toolchains(toolchains: &mut [ToolchainInfo]) {
    fn channel_preference(version: &RustVersion) -> u8 {
        match version.channel() {
            Some(Channel::Nightly) => 2,
            Some(Channel::Beta) => 1,
            // Exact release toolchains do not carry a channel name.
            Some(Channel::Stable) | None => 0,
        }
    }

    toolchains.sort_unstable_by(|a, b| {
        channel_preference(&a.version)
            .cmp(&channel_preference(&b.version))
            .then_with(|| b.version.cmp(&a.version))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.path.cmp(&b.path))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toolchain(name: &str, version: &semver::Version, toolchain_name: &str) -> ToolchainInfo {
        let path = PathBuf::from("/rustup/toolchains").join(toolchain_name);
        ToolchainInfo {
            name: name.to_string(),
            version: RustVersion::from_path(version, &path),
            path,
        }
    }

    #[test]
    fn preferred_toolchain_order_prefers_release_over_newer_nightly() {
        let mut toolchains = vec![
            toolchain(
                "nightly-aarch64-apple-darwin",
                &semver::Version::new(1, 97, 0),
                "nightly-aarch64-apple-darwin",
            ),
            toolchain(
                "stable-aarch64-apple-darwin",
                &semver::Version::new(1, 90, 0),
                "stable-aarch64-apple-darwin",
            ),
        ];

        sort_toolchains(&mut toolchains);
        let selected = toolchains
            .into_iter()
            .find(|info| RustRequest::Any.matches(&info.version, Some(&info.path)))
            .expect("matching toolchain");

        assert_eq!(selected.name, "stable-aarch64-apple-darwin");
    }

    #[test]
    fn preferred_toolchain_order_uses_nightly_when_it_is_the_only_candidate() {
        let mut toolchains = vec![toolchain(
            "nightly-aarch64-apple-darwin",
            &semver::Version::new(1, 97, 0),
            "nightly-aarch64-apple-darwin",
        )];

        sort_toolchains(&mut toolchains);
        let selected = toolchains
            .into_iter()
            .find(|info| RustRequest::Any.matches(&info.version, Some(&info.path)))
            .expect("matching toolchain");

        assert_eq!(selected.name, "nightly-aarch64-apple-darwin");
    }
}
