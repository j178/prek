use std::env;
use std::path::PathBuf;

use anyhow::Result;
use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType, UpdateRequest};
use tracing::{debug, enabled, trace, warn};

use crate::fs::LockedFile;
use crate::store::Store;

// The version of `uv` to install. Should update periodically.
const UV_VERSION: &str = "0.5.2";

// TODO: allow opt-out uv
// TODO: allow install uv using pip

/// Ensure that the `uv` binary is available.
pub(crate) async fn ensure_uv() -> Result<PathBuf> {
    // 1) Check if `uv` is installed already.
    if let Ok(uv) = which::which("uv") {
        trace!(uv = %uv.display(), "Found uv from PATH");
        return Ok(uv);
    }

    // 2) Check if `uv` is installed by `prefligit`
    let store = Store::from_settings()?;

    let uv_dir = store.uv_path();
    let uv = uv_dir.join("uv").with_extension(env::consts::EXE_EXTENSION);
    if uv.is_file() {
        trace!(uv = %uv.display(), "Found managed uv");
        return Ok(uv);
    }

    fs_err::create_dir_all(&uv_dir)?;
    let _lock = LockedFile::acquire(uv_dir.join(".lock"), "uv").await?;

    if uv.is_file() {
        trace!(uv = %uv.display(), "Found managed uv");
        return Ok(uv);
    }

    // 3) Download and install `uv`
    let mut installer = AxoUpdater::new_for("uv");
    installer.configure_version_specifier(UpdateRequest::SpecificTag(UV_VERSION.to_string()));
    installer.always_update(true);
    installer.set_install_dir(&uv_dir.to_string_lossy());
    installer.set_release_source(ReleaseSource {
        release_type: ReleaseSourceType::GitHub,
        owner: "astral-sh".to_string(),
        name: "uv".to_string(),
        app_name: "uv".to_string(),
    });
    if enabled!(tracing::Level::DEBUG) {
        installer.enable_installer_output();
        env::set_var("INSTALLER_PRINT_VERBOSE", "1");
    } else {
        installer.disable_installer_output();
    }
    // We don't want the installer to modify the PATH, and don't need the receipt.
    env::set_var("UV_UNMANAGED_INSTALL", "1");

    match installer.run().await {
        Ok(Some(result)) => {
            debug!(
                uv = %uv.display(),
                version = result.new_version_tag,
                "Successfully installed uv"
            );
            Ok(uv)
        }
        Ok(None) => Ok(uv),
        Err(err) => {
            warn!(?err, "Failed to install uv");
            Err(err.into())
        }
    }
}
