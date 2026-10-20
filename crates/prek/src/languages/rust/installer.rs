use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use futures_util::StreamExt;
use itertools::Itertools;
use semver::Version;
use tracing::{debug, trace, warn};

use crate::fs::LockedFile;
use crate::languages::rust::RustRequest;
use crate::languages::rust::rustup::Rustup;
use crate::languages::rust::toolchain::RustToolchain;
use crate::languages::rust::version::Channel;
use crate::languages::version::{ToolchainPolicy, ToolchainSource};
use crate::store::{Store, ToolBucket};

/// Selects or installs a toolchain according to the requested version and source policy.
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
                ToolchainSource::Managed => self.find_managed(request).await?,
                ToolchainSource::System => self.find_system(request).await?,
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

    async fn find_managed(&self, request: &RustRequest) -> Result<Option<RustToolchain>> {
        let paths = self.list_managed_toolchains().await?;

        Ok(Self::select_toolchain(paths, request, None).await)
    }

    async fn list_managed_toolchains(&self) -> Result<Vec<PathBuf>> {
        let mut dirs = match fs_err::tokio::read_dir(self.root.join("toolchains")).await {
            Ok(dirs) => dirs,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };

        let mut paths = Vec::new();
        while let Some(entry) = dirs.next_entry().await? {
            paths.push(entry.path());
        }

        Ok(paths)
    }

    async fn find_system(&self, request: &RustRequest) -> Result<Option<RustToolchain>> {
        let managed_root = dunce::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let rustc_paths = which::which_all("rustc").into_iter().flatten();

        for rustc in rustc_paths {
            let result = self
                .inspect_system_rust(&rustc, request, &managed_root)
                .await;

            match result {
                Ok(Some(rust)) => return Ok(Some(rust)),
                Ok(None) => {}
                Err(err) => debug!("Skipping unusable system Rust: {err:#}"),
            }
        }

        if let Some(rustup) = Rustup::find(&self.root) {
            let paths = rustup.list_toolchains().await?;

            return Ok(Self::select_toolchain(paths, request, Some(&managed_root)).await);
        }

        Ok(None)
    }

    async fn inspect_system_rust(
        &self,
        rustc: &Path,
        request: &RustRequest,
        managed_root: &Path,
    ) -> Result<Option<RustToolchain>> {
        let rustc = Rustup::resolve_rustc(rustc, self.cwd).await?;
        if dunce::canonicalize(&rustc)?.starts_with(managed_root) {
            return Ok(None);
        }

        let cargo = rustc.with_file_name("cargo").with_extension(EXE_EXTENSION);
        if !cargo.is_file() {
            return Ok(None);
        }

        let version = RustToolchain::query_version(&rustc).await?;
        if !request.matches(&version) {
            return Ok(None);
        }

        Ok(Some(RustToolchain { rustc, version }))
    }

    async fn select_toolchain(
        paths: impl IntoIterator<Item = PathBuf>,
        request: &RustRequest,
        excluded_root: Option<&Path>,
    ) -> Option<RustToolchain> {
        let mut toolchains: Vec<_> = futures_util::stream::iter(paths)
            .map(async move |path| RustToolchain::inspect_dir(&path).await)
            .buffer_unordered(8)
            .filter_map(async move |result| match result {
                Ok(toolchain) => Some(toolchain),
                Err(err) => {
                    warn!("Skipping invalid toolchain: {err:#}");
                    None
                }
            })
            .collect()
            .await;

        sort_toolchains(&mut toolchains);

        for toolchain in toolchains {
            if let Some(root) = excluded_root {
                let rustc = dunce::canonicalize(&toolchain.rustc)
                    .unwrap_or_else(|_| toolchain.rustc.clone());
                if rustc.starts_with(root) {
                    continue;
                }
            }

            if !request.matches(&toolchain.version) {
                continue;
            }

            return Some(toolchain);
        }

        None
    }

    async fn download(&self, request: &RustRequest) -> Result<RustToolchain> {
        let rustup = Rustup::install(self.store, &self.root).await?;

        fs_err::tokio::create_dir_all(&self.root).await?;
        let _lock = LockedFile::acquire(self.root.join(".lock"), "rustup").await?;

        // Another process may have installed a matching toolchain while we waited.
        if let Some(toolchain) = self.find_managed(request).await? {
            return Ok(toolchain);
        }

        let toolchain = Self::resolve_version(request).await?;
        debug!(%toolchain, "Installing Rust toolchain");

        let path = rustup.install_toolchain(&self.root, &toolchain).await?;

        RustToolchain::inspect_dir(&path).await
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
            Ok(RustToolchain {
                rustc: PathBuf::from("rustc"),
                version: Version::parse(release)?,
            })
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
