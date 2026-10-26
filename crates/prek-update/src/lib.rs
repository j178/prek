//! Self-update support for standalone prek installations.
//!
//! Adapted from axoupdater 0.10.2. See LICENSE.axoupdater for its license.

mod github;
mod receipt;

#[cfg(test)]
mod tests;

use std::process::Stdio;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use semver::Version;
use tokio::process::Command;

use crate::receipt::Receipt;

/// A standalone installation described by a loaded cargo-dist receipt.
pub struct Updater {
    receipt: Receipt,
}

/// The release to install.
#[derive(Clone, Copy)]
pub enum UpdateRequest<'a> {
    /// Upgrade to the latest stable release, without downgrading.
    Latest,
    /// Install a particular tag, allowing downgrades and prereleases.
    SpecificTag(&'a str),
}

/// The versions before and after a successful installer invocation.
pub struct UpdateResult {
    pub old_version: Version,
    pub new_version: Version,
    pub new_version_tag: String,
}

impl Updater {
    /// Load prek's standalone install receipt from the installer configuration directory.
    pub fn load_receipt() -> Result<Self> {
        Ok(Self {
            receipt: Receipt::load()?,
        })
    }

    /// Check that this receipt describes the running executable's installation directory.
    pub fn check_receipt_is_for_this_executable(&self) -> Result<bool> {
        self.receipt.matches_executable(&std::env::current_exe()?)
    }

    /// Run the release's standalone installer if the requested version differs.
    ///
    /// Returns `None` if this receipt belongs to another installation or no update is needed.
    /// The supplied client controls TLS, proxies, and timeouts. The token authenticates GitHub
    /// API requests; it is not forwarded to asset download URLs.
    pub async fn run(
        &self,
        client: &Client,
        request: UpdateRequest<'_>,
        token: Option<&str>,
        verbose: bool,
    ) -> Result<Option<UpdateResult>> {
        if !self.check_receipt_is_for_this_executable()? {
            return Ok(None);
        }

        let release = github::fetch_release(client, &self.receipt.source, request, token).await?;
        let needs_update = match request {
            UpdateRequest::Latest => self.receipt.version < release.version,
            UpdateRequest::SpecificTag(_) => self.receipt.version != release.version,
        };
        if !needs_update {
            return Ok(None);
        }

        let extension = if cfg!(windows) { "ps1" } else { "sh" };
        let installer_name = format!("prek-installer.{extension}");
        let asset = release
            .assets
            .iter()
            .find(|asset| asset.name == installer_name)
            .context("This release does not contain a prek installer for this platform")?;
        let download = client
            .get(&asset.browser_download_url)
            .header(reqwest::header::ACCEPT, "application/octet-stream")
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        let directory = tempfile::tempdir()?;
        let installer = directory.path().join(format!("installer.{extension}"));
        fs_err::tokio::write(&installer, download).await?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs_err::set_permissions(&installer, std::fs::Permissions::from_mode(0o744))?;
        }

        let mut command = if cfg!(windows) {
            let mut command = Command::new("powershell");
            command.args(["-ExecutionPolicy", "ByPass"]).arg(&installer);
            command
        } else {
            Command::new(&installer)
        };
        if verbose {
            command
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .env("INSTALLER_PRINT_VERBOSE", "1");
        }
        // PowerShell Core's module path can prevent the Windows installer from loading modules.
        command.env_remove("PSModulePath");
        let install_prefix = self.receipt.install_prefix_root()?;
        command
            .env("CARGO_DIST_FORCE_INSTALL_DIR", install_prefix)
            .env("PREK_INSTALL_DIR", install_prefix);
        if !self.receipt.modify_path {
            command.env("PREK_NO_MODIFY_PATH", "1");
        }

        // Windows cannot overwrite a running executable. Restore it if the installer fails.
        #[cfg(windows)]
        let previous = {
            let executable = std::env::current_exe()?;
            let mut previous = executable.as_os_str().to_os_string();
            previous.push(".previous.exe");
            let previous = std::path::PathBuf::from(previous);
            fs_err::rename(&executable, &previous)?;
            (executable, previous)
        };

        let result = command.output().await;
        #[cfg(windows)]
        {
            let (executable, previous) = previous;
            if result.as_ref().is_ok_and(|output| output.status.success()) {
                self_replace::self_delete_at(&previous)
                    .context("Failed to clean up the previous prek executable")?;
            } else {
                fs_err::rename(&previous, &executable)
                    .context("Failed to restore the previous prek executable")?;
            }
        }

        let output = result.context("Failed to execute the prek installer")?;
        if !output.status.success() {
            bail!(
                "The installation failed ({}). Output from the installer:\n{}{}",
                output.status,
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            );
        }

        Ok(Some(UpdateResult {
            old_version: self.receipt.version.clone(),
            new_version: release.version,
            new_version_tag: release.tag_name,
        }))
    }
}
