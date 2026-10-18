use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use itertools::Itertools;
use semver::Version;
use tracing::{debug, trace};

use crate::fs::LockedFile;
use crate::languages::rust::RustRequest;
use crate::languages::rust::rustup::Rustup;
use crate::languages::rust::toolchain::{RustToolchain, resolve_proxy};
use crate::languages::rust::version::{Channel, query_version};
use crate::languages::version::{ToolchainPolicy, ToolchainSource, find_system_executables};
use crate::store::{Store, ToolBucket};

pub(crate) struct RustInstaller<'a> {
    store: &'a Store,
    root: PathBuf,
    cwd: &'a Path,
}

impl<'a> RustInstaller<'a> {
    pub(crate) fn new(store: &'a Store, cwd: &'a Path) -> Self {
        Self {
            store,
            root: store.tools_path(ToolBucket::Rustup),
            cwd,
        }
    }

    pub(crate) async fn install(
        &self,
        request: &RustRequest,
        policy: ToolchainPolicy,
    ) -> Result<RustToolchain> {
        for &source in policy.search_order() {
            let result = match source {
                ToolchainSource::Managed => self.find_installed(request).await?,
                ToolchainSource::System => self.find_system_rust(request).await?,
            };
            if let Some(result) = result {
                trace!(?result, ?source, "Found rust");
                return Ok(result);
            }
        }

        if !policy.allows_download() {
            anyhow::bail!("No suitable Rust version found for toolchain policy: {policy}");
        }

        self.download(request).await
    }

    async fn find_installed(&self, request: &RustRequest) -> Result<Option<RustToolchain>> {
        let toolchains = Rustup::list_installed_toolchains(&self.root).await?;
        Ok(Self::select_toolchain(toolchains, request).await)
    }

    async fn select_toolchain(
        mut toolchains: Vec<RustToolchain>,
        request: &RustRequest,
    ) -> Option<RustToolchain> {
        sort_toolchains(&mut toolchains);
        for toolchain in toolchains {
            if !request.matches(&toolchain.version) {
                continue;
            }
            if let Err(err) = toolchain.check_cargo().await {
                debug!("Skipping unusable Rust toolchain: {err:#}");
                continue;
            }
            return Some(toolchain);
        }
        None
    }

    async fn find_system_rust(&self, request: &RustRequest) -> Result<Option<RustToolchain>> {
        let rustc_paths = find_system_executables("rustc", &self.root).unwrap_or_default();
        for rustc in rustc_paths {
            let result = self.inspect_system_rust(&rustc, request).await;
            match result {
                Ok(Some(rust)) => return Ok(Some(rust)),
                Ok(None) => {}
                Err(err) => debug!("Skipping unusable system Rust: {err:#}"),
            }
        }

        if let Some(rustup) = Rustup::find(&self.root) {
            let toolchains = rustup.list_system_toolchains().await?;
            let managed_root =
                dunce::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
            let toolchains = toolchains
                .into_iter()
                .filter(|info| {
                    let path =
                        dunce::canonicalize(&info.rustc).unwrap_or_else(|_| info.rustc.clone());
                    !path.starts_with(&managed_root)
                })
                .collect();
            return Ok(Self::select_toolchain(toolchains, request).await);
        }
        Ok(None)
    }

    async fn inspect_system_rust(
        &self,
        rustc: &Path,
        request: &RustRequest,
    ) -> Result<Option<RustToolchain>> {
        let rustc = resolve_proxy(rustc, "rustc", self.cwd).await?;
        let managed_root = dunce::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        if dunce::canonicalize(&rustc)?.starts_with(&managed_root) {
            return Ok(None);
        }
        let version = query_version(&rustc).await?;
        if !request.matches(&version) {
            return Ok(None);
        }
        Ok(RustToolchain::find_cargo(&rustc, &version, self.cwd, Some(&managed_root)).await)
    }

    async fn resolve_version(req: &RustRequest) -> Result<String> {
        match req {
            RustRequest::Any => Ok("stable".to_owned()),
            RustRequest::Channel(ch) => Ok(ch.to_string()),

            RustRequest::Major(_)
            | RustRequest::MajorMinor(_, _)
            | RustRequest::MajorMinorPatch(_, _, _)
            | RustRequest::Range(_, _) => {
                let output = crate::git::git_cmd()?
                    .arg("ls-remote")
                    .arg("--tags")
                    .arg("https://github.com/rust-lang/rust")
                    .output()
                    .await?
                    .stdout;
                let version = str::from_utf8(&output)?
                    .lines()
                    .filter_map(|line| {
                        let tag = line.split('\t').nth(1)?;
                        let tag = tag.strip_prefix("refs/tags/")?;
                        Version::parse(tag).ok()
                    })
                    .sorted_unstable_by(|a, b| b.cmp(a))
                    .find(|version| req.matches(version))
                    .with_context(|| format!("Version `{req}` not found on remote"))?;
                Ok(version.to_string())
            }
        }
    }

    async fn download(&self, request: &RustRequest) -> Result<RustToolchain> {
        let rustup = Rustup::install(self.store, &self.root).await?;
        fs_err::tokio::create_dir_all(&self.root).await?;
        let _lock = LockedFile::acquire(self.root.join(".lock"), "rustup").await?;
        // Another process may have installed a matching toolchain while we waited.
        if let Some(toolchain) = self.find_installed(request).await? {
            return Ok(toolchain);
        }
        let toolchain = Self::resolve_version(request).await?;
        debug!(%toolchain, "Installing Rust toolchain");
        let toolchain = rustup.install_toolchain(&toolchain).await?;
        toolchain.check_cargo().await?;
        Ok(toolchain)
    }
}

fn sort_toolchains(toolchains: &mut [RustToolchain]) {
    fn channel_preference(version: &Version) -> u8 {
        match Channel::from_version(version) {
            Some(Channel::Nightly) => 2,
            Some(Channel::Beta) => 1,
            Some(Channel::Stable) => 0,
            None => 3,
        }
    }

    toolchains.sort_unstable_by(|a, b| {
        channel_preference(&a.version)
            .cmp(&channel_preference(&b.version))
            .then_with(|| b.version.cmp(&a.version))
            .then_with(|| a.rustc.cmp(&b.rustc))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preferred_toolchain_order() -> Result<()> {
        let mut toolchains = [
            "1.99.0-dev",
            "1.97.0-nightly",
            "1.96.0-beta.1",
            "1.90.0",
            "1.95.0",
        ]
        .into_iter()
        .map(|release| {
            Ok(RustToolchain::from_dir(
                Path::new("toolchain"),
                Version::parse(release)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;

        sort_toolchains(&mut toolchains);
        assert_eq!(
            toolchains
                .iter()
                .map(|toolchain| toolchain.version.to_string())
                .collect::<Vec<_>>(),
            [
                "1.95.0",
                "1.90.0",
                "1.96.0-beta.1",
                "1.97.0-nightly",
                "1.99.0-dev"
            ]
        );
        Ok(())
    }
}
