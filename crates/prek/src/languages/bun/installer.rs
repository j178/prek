use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use asyncband::once::OnceMap;
use itertools::Itertools;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use rustc_hash::FxBuildHasher;
use target_lexicon::{Architecture, HOST, OperatingSystem, Triple};
use tracing::{debug, trace, warn};

use crate::archive;
use crate::checksum::{digest_from_sha256sums, fetch_checksum};
use crate::fs::LockedFile;
use crate::git;
use crate::http::download_artifact;
use crate::languages::bun::BunRequest;
use crate::languages::bun::version::BunVersion;
use crate::languages::version::{ToolchainPolicy, ToolchainSource, find_system_executables};
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug)]
pub(crate) struct BunResult {
    bun: PathBuf,
    version: BunVersion,
}

impl Display for BunResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.bun.display(), self.version)?;
        Ok(())
    }
}

/// Override the Bun binary name for testing.
static BUN_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars.var(EnvVars::PREK_INTERNAL__BUN_BINARY_NAME) {
        name
    } else {
        "bun".to_string()
    }
});

impl BunResult {
    pub(crate) fn from_dir(dir: &Path, version: BunVersion) -> Self {
        let bun = bin_dir(dir).join("bun").with_extension(EXE_EXTENSION);
        Self { bun, version }
    }

    pub(crate) async fn from_executable(bun: PathBuf) -> Result<Self> {
        static VERSIONS: LazyLock<OnceMap<PathBuf, BunVersion, FxBuildHasher>> =
            LazyLock::new(|| OnceMap::with_hasher(FxBuildHasher));

        let key = fs_err::canonicalize(&bun).unwrap_or_else(|_| bun.clone());
        let version = VERSIONS
            .try_compute(key, async || {
                let output = Cmd::new(&bun).arg("--version").check(true).output().await?;
                let output_str = str::from_utf8(&output.stdout)?;
                output_str
                    .trim()
                    .parse()
                    .context("Failed to parse bun version")
            })
            .await?;

        Ok(Self { bun, version })
    }

    pub(crate) fn bun(&self) -> &Path {
        &self.bun
    }

    pub(crate) fn version(&self) -> &BunVersion {
        &self.version
    }
}

pub(crate) struct BunInstaller {
    root: PathBuf,
}

impl BunInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Install a version of Bun.
    pub(crate) async fn install(
        &self,
        store: &Store,
        request: &BunRequest,
        policy: ToolchainPolicy,
    ) -> Result<BunResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;

        let _lock = LockedFile::acquire(self.root.join(".lock"), "bun").await?;

        for &source in policy.search_order() {
            let result = match source {
                ToolchainSource::Managed => self.find_installed(request).ok(),
                ToolchainSource::System => self.find_system_bun(request).await?,
            };
            if let Some(result) = result {
                trace!(%result, ?source, "Found bun");
                return Ok(result);
            }
        }

        if !policy.allows_download() {
            anyhow::bail!("No suitable Bun version found for toolchain policy: {policy}");
        }

        let resolved_version = self.resolve_version(request).await?;
        trace!(version = %resolved_version, "Downloading bun");

        self.download(store, &resolved_version).await
    }

    /// Get the installed version of Bun.
    fn find_installed(&self, req: &BunRequest) -> Result<BunResult> {
        let mut installed = fs_err::read_dir(&self.root)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(err) => {
                    warn!(?err, "Failed to read entry");
                    None
                }
            })
            .filter(|entry| entry.file_type().is_ok_and(|f| f.is_dir()))
            .filter_map(|entry| {
                let dir_name = entry.file_name();
                let version = BunVersion::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| a.cmp(b))
            .rev();

        installed
            .find_map(|(v, path)| {
                if req.matches(&v) {
                    Some(BunResult::from_dir(&path, v))
                } else {
                    None
                }
            })
            .context("No installed bun version matches the request")
    }

    async fn resolve_version(&self, req: &BunRequest) -> Result<BunVersion> {
        // Latest versions come first, so we can find the latest matching version.
        let versions = self
            .list_remote_versions()
            .await
            .context("Failed to list remote versions")?;
        let version = versions
            .into_iter()
            .find(|version| req.matches(version))
            .context("Version not found on remote")?;
        Ok(version)
    }

    /// List all versions of Bun available on GitHub releases.
    async fn list_remote_versions(&self) -> Result<Vec<BunVersion>> {
        let output = git::git_cmd()?
            .arg("ls-remote")
            .arg("--tags")
            .arg("https://github.com/oven-sh/bun")
            .output()
            .await?
            .stdout;
        let output_str = str::from_utf8(&output)?;

        let versions: Vec<BunVersion> = output_str
            .lines()
            .filter_map(|line| {
                let reference = line.split('\t').nth(1)?;
                if reference.ends_with("^{}") {
                    return None;
                }

                let tag = reference.strip_prefix("refs/tags/")?;
                // Tags are in format "bun-v1.1.0".
                let tag = tag.strip_prefix("bun-v")?;
                BunVersion::from_str(tag).ok()
            })
            .sorted_unstable_by(|a, b| b.cmp(a))
            .collect();

        Ok(versions)
    }

    /// Install a specific version of Bun.
    async fn download(&self, store: &Store, version: &BunVersion) -> Result<BunResult> {
        let filename = release_filename(&HOST, has_glibc(&HOST).await?)?;
        let url =
            format!("https://github.com/oven-sh/bun/releases/download/bun-v{version}/{filename}");
        let checksum_url = format!(
            "https://github.com/oven-sh/bun/releases/download/bun-v{version}/SHASUMS256.txt"
        );
        let target = self.root.join(version.to_string());

        let download = download_artifact(&url, &filename, store, async || {
            let Some(checksums) = fetch_checksum(&checksum_url).await? else {
                return Ok(None);
            };
            digest_from_sha256sums(&checksums, &filename)
        })
        .await
        .context("Failed to download bun")?;
        let extracted = archive::extract_archive(download.path())
            .await
            .context("Failed to extract bun")?;
        if target.exists() {
            debug!(target = %target.display(), "Removing existing bun");
            fs_err::tokio::remove_dir_all(&target).await?;
        }

        // Move the binary from the archive's top-level directory to {version}/bin/bun.
        let extracted_binary = extracted.join("bun").with_extension(EXE_EXTENSION);
        let target_bin_dir = bin_dir(&target);
        fs_err::tokio::create_dir_all(&target_bin_dir).await?;

        let target_binary = target_bin_dir.join("bun").with_extension(EXE_EXTENSION);
        debug!(?extracted_binary, target = %target_binary.display(), "Moving bun to target");
        crate::fs::rename_with_retry(&extracted_binary, &target_binary).await?;

        Ok(BunResult::from_dir(&target, version.clone()))
    }

    /// Find a suitable system Bun installation that matches the request.
    async fn find_system_bun(&self, bun_request: &BunRequest) -> Result<Option<BunResult>> {
        let bun_paths = match find_system_executables(&*BUN_BINARY_NAME, &self.root) {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No bun executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        // Check each bun executable for a matching version, stop early if found
        for bun_path in bun_paths {
            match BunResult::from_executable(bun_path).await {
                Ok(bun_result) => {
                    // Check if this version matches the request
                    if bun_request.matches(&bun_result.version) {
                        trace!(
                            %bun_result,
                            "Found a matching system bun"
                        );
                        return Ok(Some(bun_result));
                    }
                    trace!(
                        %bun_result,
                        "System bun does not match requested version"
                    );
                }
                Err(e) => {
                    warn!(?e, "Failed to get version for system bun");
                }
            }
        }

        debug!(?bun_request, "No system bun matches the requested version");
        Ok(None)
    }
}

async fn has_glibc(host: &Triple) -> Result<bool> {
    // Static musl builds of prek can also run on glibc systems. Check the loader
    // Bun needs at runtime, not the libc prek was compiled against.
    let loader = match (host.operating_system, host.architecture) {
        (OperatingSystem::Linux, Architecture::X86_64) => "/lib64/ld-linux-x86-64.so.2",
        (OperatingSystem::Linux, Architecture::Aarch64(_)) => "/lib/ld-linux-aarch64.so.1",
        _ => return Ok(false),
    };
    probe_glibc_loader(Path::new(loader)).await
}

async fn probe_glibc_loader(loader: &Path) -> Result<bool> {
    if !loader
        .try_exists()
        .with_context(|| format!("Failed to check for glibc loader at {}", loader.display()))?
    {
        return Ok(false);
    }

    // gcompat may occupy this path, but rejects direct invocation. Check only
    // the exit status: glibc's version banner can be distro-branded, and
    // --list-tunables would exclude glibc versions older than 2.33.
    let output = Cmd::new(loader)
        .arg("--version")
        .check(false)
        .output()
        .await
        .context("Failed to query glibc loader version")?;
    let version = String::from_utf8_lossy(&output.stdout);
    let available = output.status.success();
    debug!(loader = %loader.display(), %version, glibc_available = available, "Detected Bun runtime libc");
    Ok(available)
}

fn release_filename(host: &Triple, glibc_available: bool) -> Result<String> {
    let arch = match host.architecture {
        Architecture::X86_64 => "x64",
        Architecture::Aarch64(_) => "aarch64",
        _ => anyhow::bail!("Unsupported architecture"),
    };
    let os = match host.operating_system {
        OperatingSystem::Darwin(_) => "darwin",
        OperatingSystem::Linux => "linux",
        OperatingSystem::Windows => "windows",
        _ => anyhow::bail!("Unsupported OS"),
    };

    let suffix = if host.operating_system == OperatingSystem::Linux && !glibc_available {
        "-musl"
    } else {
        ""
    };

    Ok(format!("bun-{os}-{arch}{suffix}.zip"))
}

pub(crate) fn bin_dir(prefix: &Path) -> PathBuf {
    // Bun installs global packages to $BUN_INSTALL/bin/ on all platforms
    prefix.join("bin")
}

pub(crate) fn lib_dir(prefix: &Path) -> PathBuf {
    if cfg!(windows) {
        prefix.join("node_modules")
    } else {
        prefix.join("lib").join("node_modules")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Missing loaders and failed probes select musl, regardless of banner text.
    #[cfg(unix)]
    #[tokio::test]
    async fn glibc_loader_detection() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let loader = temp.path().join("ld.so");
        assert!(!probe_glibc_loader(&loader).await?);

        for (version, exit_code, expected) in [
            ("ld.so (GNU libc) stable release version 2.41.", 0, true),
            (
                "ld.so (Debian GLIBC 2.41) stable release version 2.41.",
                0,
                true,
            ),
            (
                "ld.so (Gentoo 2.39-r6 p4) stable release version 2.39.",
                0,
                true,
            ),
            ("musl libc", 1, false),
            ("This is the gcompat ELF interpreter stub.", 1, false),
            ("GNU libc", 1, false),
        ] {
            fs_err::write(
                &loader,
                format!("#!/bin/sh\nprintf '%s\\n' '{version}'\nexit {exit_code}\n"),
            )?;
            crate::fs::make_executable(&loader)?;
            assert_eq!(probe_glibc_loader(&loader).await?, expected, "{version}");
        }

        // A directory cannot be executed, even with shell fallback on macOS.
        fs_err::remove_file(&loader)?;
        fs_err::create_dir(&loader)?;
        assert!(probe_glibc_loader(&loader).await.is_err());
        Ok(())
    }

    // Both Linux architectures need musl archives when the glibc loader is absent,
    // regardless of the libc prek was compiled against.
    #[test]
    fn linux_without_glibc() -> Result<()> {
        for (triple, expected) in [
            ("x86_64-unknown-linux-musl", "bun-linux-x64-musl.zip"),
            ("aarch64-unknown-linux-musl", "bun-linux-aarch64-musl.zip"),
            ("x86_64-unknown-linux-gnu", "bun-linux-x64-musl.zip"),
            ("aarch64-unknown-linux-gnu", "bun-linux-aarch64-musl.zip"),
        ] {
            let host = triple.parse().map_err(anyhow::Error::msg)?;
            assert_eq!(release_filename(&host, false)?, expected);
        }
        Ok(())
    }

    // A musl-built prek running on glibc Linux must keep using the glibc archive.
    #[test]
    fn linux_with_glibc() -> Result<()> {
        for (triple, expected) in [
            ("x86_64-unknown-linux-gnu", "bun-linux-x64.zip"),
            ("aarch64-unknown-linux-gnu", "bun-linux-aarch64.zip"),
            ("x86_64-unknown-linux-musl", "bun-linux-x64.zip"),
            ("aarch64-unknown-linux-musl", "bun-linux-aarch64.zip"),
        ] {
            let host = triple.parse().map_err(anyhow::Error::msg)?;
            assert_eq!(release_filename(&host, true)?, expected);
        }
        Ok(())
    }

    // The libc probe must not change macOS or Windows archive names.
    #[test]
    fn non_linux_platforms() -> Result<()> {
        for (triple, expected) in [
            ("x86_64-apple-darwin", "bun-darwin-x64.zip"),
            ("aarch64-apple-darwin", "bun-darwin-aarch64.zip"),
            ("x86_64-pc-windows-msvc", "bun-windows-x64.zip"),
            ("aarch64-pc-windows-msvc", "bun-windows-aarch64.zip"),
        ] {
            let host = triple.parse().map_err(anyhow::Error::msg)?;
            for glibc_available in [false, true] {
                assert_eq!(release_filename(&host, glibc_available)?, expected);
            }
        }
        Ok(())
    }

    // Unsupported CPUs and operating systems must still fail before downloading.
    #[test]
    fn unsupported_platforms() -> Result<()> {
        for triple in ["riscv64gc-unknown-linux-gnu", "x86_64-unknown-freebsd"] {
            let host = triple.parse().map_err(anyhow::Error::msg)?;
            assert!(release_filename(&host, false).is_err());
        }
        Ok(())
    }
}
