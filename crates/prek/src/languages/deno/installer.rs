use std::env::consts::EXE_EXTENSION;
use std::fmt::Display;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::LazyLock;

use anyhow::{Context, Result};
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;
use serde::Deserialize;
use target_lexicon::{Architecture, HOST, OperatingSystem};
use tracing::{debug, trace, warn};

use crate::archive;
use crate::checksum::{Sha256Digest, digest_from_sha256sums};
use crate::fs::LockedFile;
use crate::http::{REQWEST_CLIENT, download_artifact};
use crate::languages::deno::DenoRequest;
use crate::languages::deno::version::DenoVersion;
use crate::process::Cmd;
use crate::store::Store;

#[derive(Debug)]
pub(crate) struct DenoResult {
    deno: PathBuf,
    version: DenoVersion,
}

impl Display for DenoResult {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.deno.display(), self.version)?;
        Ok(())
    }
}

/// Override the Deno binary name for testing.
static DENO_BINARY_NAME: LazyLock<String> = LazyLock::new(|| {
    if let Ok(name) = EnvVars::var(EnvVars::PREK_INTERNAL__DENO_BINARY_NAME) {
        name
    } else {
        "deno".to_string()
    }
});

impl DenoResult {
    pub(crate) fn from_executable(deno: PathBuf) -> Self {
        Self {
            deno,
            version: DenoVersion::default(),
        }
    }

    pub(crate) fn from_dir(dir: &Path) -> Self {
        let deno = bin_dir(dir).join("deno").with_extension(EXE_EXTENSION);
        Self::from_executable(deno)
    }

    pub(crate) fn with_version(mut self, version: DenoVersion) -> Self {
        self.version = version;
        self
    }

    pub(crate) async fn fill_version(mut self) -> Result<Self> {
        let output = Cmd::new(&self.deno, "deno --version")
            .env(EnvVars::DENO_NO_UPDATE_CHECK, "1")
            .arg("--version")
            .check(true)
            .output()
            .await?;
        // Output format: "deno 2.1.0 (release, x86_64-unknown-linux-gnu)\n..."
        let output_str = String::from_utf8_lossy(&output.stdout);
        let version_str = output_str
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("deno "))
            .and_then(|rest| rest.split_whitespace().next())
            .context("Failed to parse deno version output")?;

        self.version = version_str
            .parse()
            .context("Failed to parse deno version")?;

        Ok(self)
    }

    pub(crate) fn deno(&self) -> &Path {
        &self.deno
    }

    pub(crate) fn version(&self) -> &DenoVersion {
        &self.version
    }
}

pub(crate) struct DenoInstaller {
    root: PathBuf,
}

impl DenoInstaller {
    pub(crate) fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Install a version of Deno.
    pub(crate) async fn install(
        &self,
        store: &Store,
        request: &DenoRequest,
        allows_download: bool,
    ) -> Result<DenoResult> {
        fs_err::tokio::create_dir_all(&self.root).await?;

        let _lock = LockedFile::acquire(self.root.join(".lock"), "deno").await?;

        if let Ok(deno_result) = self.find_installed(request) {
            trace!(%deno_result, "Found installed deno");
            return Ok(deno_result);
        }

        // Find all deno executables in PATH and check their versions
        if let Some(deno_result) = self.find_system_deno(request).await? {
            trace!(%deno_result, "Using system deno");
            return Ok(deno_result);
        }

        if !allows_download {
            anyhow::bail!("No suitable system Deno version found and downloads are disabled");
        }

        let resolved_version = self.resolve_version(request).await?;
        trace!(version = %resolved_version, "Downloading deno");

        self.download(store, &resolved_version).await
    }

    /// Get the installed version of Deno.
    fn find_installed(&self, req: &DenoRequest) -> Result<DenoResult> {
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
                let version = DenoVersion::from_str(&dir_name.to_string_lossy()).ok()?;
                Some((version, entry.path()))
            })
            .sorted_unstable_by(|(a, _), (b, _)| a.cmp(b))
            .rev();

        installed
            .find_map(|(v, path)| {
                if req.matches(&v, Some(&path)) {
                    Some(DenoResult::from_dir(&path).with_version(v))
                } else {
                    None
                }
            })
            .context("No installed deno version matches the request")
    }

    async fn resolve_version(&self, req: &DenoRequest) -> Result<DenoVersion> {
        // Latest versions come first, so we can find the latest matching version.
        let versions = self
            .list_remote_versions()
            .await
            .context("Failed to list remote versions")?;
        let version = versions
            .into_iter()
            .find(|version| req.matches(version, None))
            .context("Version not found on remote")?;
        Ok(version)
    }

    /// List all versions of Deno available from the official versions endpoint.
    ///
    /// Uses <https://deno.com/versions.json> which is lightweight and doesn't
    /// have rate-limit issues like the GitHub API.
    async fn list_remote_versions(&self) -> Result<Vec<DenoVersion>> {
        #[derive(Deserialize)]
        struct VersionsResponse {
            cli: Vec<String>,
        }

        let url = "https://deno.com/versions.json";
        let response: VersionsResponse = REQWEST_CLIENT.get(url).send().await?.json().await?;

        // Versions are already sorted in descending order (newest first)
        let versions: Vec<DenoVersion> = response
            .cli
            .into_iter()
            .filter_map(|v| DenoVersion::from_str(&v).ok())
            .collect();

        if versions.is_empty() {
            anyhow::bail!("No Deno versions found");
        }

        Ok(versions)
    }

    /// Install a specific version of Deno.
    async fn download(&self, store: &Store, version: &DenoVersion) -> Result<DenoResult> {
        let arch = match HOST.architecture {
            Architecture::X86_64 => "x86_64",
            Architecture::Aarch64(_) => "aarch64",
            _ => anyhow::bail!("Unsupported architecture for Deno"),
        };

        let os = match HOST.operating_system {
            OperatingSystem::Darwin(_) => "apple-darwin",
            OperatingSystem::Linux => "unknown-linux-gnu",
            OperatingSystem::Windows => "pc-windows-msvc",
            _ => anyhow::bail!("Unsupported OS for Deno"),
        };

        let filename = format!("deno-{arch}-{os}.zip");
        let url = format!("https://dl.deno.land/release/v{version}/{filename}");
        let checksum_url = format!("{url}.sha256sum");
        let target = self.root.join(version.to_string());

        let download = download_artifact(&url, &filename, store, async || {
            Self::fetch_checksum(&checksum_url, &filename).await
        })
        .await
        .context("Failed to download deno")?;
        let extracted = archive::extract_archive(download.path())
            .await
            .context("Failed to extract deno")?;
        Self::install_extracted(&target, &extracted).await?;

        Ok(DenoResult::from_dir(&target).with_version(version.clone()))
    }

    async fn fetch_checksum(checksum_url: &str, filename: &str) -> Result<Option<Sha256Digest>> {
        let response = REQWEST_CLIENT
            .get(checksum_url)
            .send()
            .await
            .with_context(|| format!("Failed to fetch Deno checksum from {checksum_url}"))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }

        let checksums = response
            .error_for_status()
            .with_context(|| format!("Failed to fetch Deno checksum from {checksum_url}"))?
            .text()
            .await?;
        digest_from_deno_checksum(&checksums, filename)
    }

    async fn install_extracted(target: &Path, extracted: &Path) -> Result<()> {
        if target.exists() {
            debug!(target = %target.display(), "Removing existing deno");
            fs_err::tokio::remove_dir_all(&target).await?;
        }

        // Deno ZIP contains just the binary at the root level.
        // After strip_component, `extracted` may be the binary itself (if singular)
        // or a directory containing the binary.
        let extracted_binary = if extracted.is_file() {
            extracted.to_path_buf()
        } else {
            extracted.join("deno").with_extension(EXE_EXTENSION)
        };

        let target_bin_dir = bin_dir(target);
        fs_err::tokio::create_dir_all(&target_bin_dir).await?;

        let target_binary = target_bin_dir.join("deno").with_extension(EXE_EXTENSION);
        debug!(?extracted_binary, target = %target_binary.display(), "Moving deno to target");
        fs_err::tokio::rename(&extracted_binary, &target_binary).await?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = fs_err::tokio::metadata(&target_binary).await?.permissions();
            perms.set_mode(0o755);
            fs_err::tokio::set_permissions(&target_binary, perms).await?;
        }

        Ok(())
    }

    /// Find a suitable system Deno installation that matches the request.
    async fn find_system_deno(&self, deno_request: &DenoRequest) -> Result<Option<DenoResult>> {
        let deno_paths = match which::which_all(&*DENO_BINARY_NAME) {
            Ok(paths) => paths,
            Err(e) => {
                debug!("No deno executables found in PATH: {}", e);
                return Ok(None);
            }
        };

        // Check each deno executable for a matching version, stop early if found
        for deno_path in deno_paths {
            match DenoResult::from_executable(deno_path).fill_version().await {
                Ok(deno_result) => {
                    // Check if this version matches the request
                    if deno_request.matches(&deno_result.version, Some(&deno_result.deno)) {
                        trace!(
                            %deno_result,
                            "Found a matching system deno"
                        );
                        return Ok(Some(deno_result));
                    }
                    trace!(
                        %deno_result,
                        "System deno does not match requested version"
                    );
                }
                Err(e) => {
                    warn!(?e, "Failed to get version for system deno");
                }
            }
        }

        debug!(
            ?deno_request,
            "No system deno matches the requested version"
        );
        Ok(None)
    }
}

pub(crate) fn bin_dir(prefix: &Path) -> PathBuf {
    prefix.join("bin")
}

fn digest_from_deno_checksum(contents: &str, filename: &str) -> Result<Option<Sha256Digest>> {
    // Deno releases have shipped per-artifact `.sha256sum` assets since v2.0.0,
    // while `dl.deno.land/release/v{version}/{filename}.sha256sum` starts at v2.0.1.
    // Non-Windows artifacts use the regular sha256sums format handled by
    // `digest_from_sha256sums`; Windows artifacts use this `Get-FileHash` shape:
    //
    // Algorithm : SHA256
    // Hash      : D45377511968CB2DB7E57155257FF9A1400DB169256B5EAA16C6657F275E4D3B
    // Path      : C:\a\deno\deno\target\release\deno-x86_64-pc-windows-msvc.zip
    if let Some(digest) = digest_from_sha256sums(contents, filename)? {
        return Ok(Some(digest));
    }
    digest_from_deno_windows_checksum(contents, filename)
}

fn digest_from_deno_windows_checksum(
    contents: &str,
    filename: &str,
) -> Result<Option<Sha256Digest>> {
    let mut algorithm = None;
    let mut hash = None;
    let mut path = None;
    for line in contents.lines() {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        match name.trim() {
            "Algorithm" => algorithm = Some(value.trim()),
            "Hash" => hash = Some(value.trim()),
            "Path" => path = Some(value.trim()),
            _ => {}
        }
    }

    if algorithm.is_none() && hash.is_none() && path.is_none() {
        return Ok(None);
    }

    let Some(algorithm) = algorithm else {
        anyhow::bail!("No algorithm found in Deno checksum file");
    };
    if !algorithm.eq_ignore_ascii_case("SHA256") {
        anyhow::bail!("Deno checksum file uses `{algorithm}` instead of `SHA256`");
    }

    let Some(path) = path else {
        anyhow::bail!("No path found in Deno checksum file");
    };
    let found_filename = path
        .rsplit(['/', '\\'])
        .next()
        .filter(|name| !name.is_empty())
        .context("No filename found in Deno checksum path")?;
    if found_filename != filename {
        return Ok(None);
    }

    let Some(hash) = hash else {
        anyhow::bail!("No hash found in Deno checksum file");
    };
    hash.parse().map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deno_sha256sum_format() -> Result<()> {
        let digest = require_deno_checksum(
            "f5b681529a0360e7f430688fc0ba3b25626b3933c370b053a0c27f39ef7a0f90  deno-x86_64-unknown-linux-gnu.zip",
            "deno-x86_64-unknown-linux-gnu.zip",
        )?;

        assert_eq!(
            digest.to_string(),
            "f5b681529a0360e7f430688fc0ba3b25626b3933c370b053a0c27f39ef7a0f90"
        );
        Ok(())
    }

    #[test]
    fn parses_deno_windows_checksum_format() -> Result<()> {
        let digest = require_deno_checksum(
            indoc::indoc! {r"
                Algorithm : SHA256
                Hash      : 1611FB54C3BB0A605A851530A359A48B34A8ABFD29D7091538D91B6E7F105380
                Path      : C:\a\deno\deno\target\release\deno-x86_64-pc-windows-msvc.zip
            "},
            "deno-x86_64-pc-windows-msvc.zip",
        )?;

        assert_eq!(
            digest.to_string(),
            "1611fb54c3bb0a605a851530a359a48b34a8abfd29d7091538d91b6e7f105380"
        );
        Ok(())
    }

    #[test]
    fn returns_none_for_deno_windows_checksum_with_different_file() -> Result<()> {
        let digest = digest_from_deno_checksum(
            indoc::indoc! {r"
                Algorithm : SHA256
                Hash      : 1611FB54C3BB0A605A851530A359A48B34A8ABFD29D7091538D91B6E7F105380
                Path      : C:\a\deno\deno\target\release\deno-x86_64-pc-windows-msvc.zip
            "},
            "deno-aarch64-pc-windows-msvc.zip",
        )?;

        assert_eq!(digest, None);
        Ok(())
    }

    #[test]
    fn rejects_deno_windows_checksum_with_different_algorithm() {
        let result = digest_from_deno_checksum(
            indoc::indoc! {r"
                Algorithm : SHA512
                Hash      : 1611FB54C3BB0A605A851530A359A48B34A8ABFD29D7091538D91B6E7F105380
                Path      : C:\a\deno\deno\target\release\deno-x86_64-pc-windows-msvc.zip
            "},
            "deno-x86_64-pc-windows-msvc.zip",
        );

        assert_eq!(
            result.unwrap_err().to_string(),
            "Deno checksum file uses `SHA512` instead of `SHA256`"
        );
    }

    #[test]
    fn returns_none_for_non_matching_sha256sums_file() -> Result<()> {
        let digest = digest_from_deno_checksum(
            "f5b681529a0360e7f430688fc0ba3b25626b3933c370b053a0c27f39ef7a0f90  deno-x86_64-unknown-linux-gnu.zip",
            "deno-aarch64-unknown-linux-gnu.zip",
        )?;

        assert_eq!(digest, None);
        Ok(())
    }

    fn require_deno_checksum(contents: &str, filename: &str) -> Result<Sha256Digest> {
        digest_from_deno_checksum(contents, filename)?
            .with_context(|| format!("No SHA256 digest found for `{filename}`"))
    }
}
