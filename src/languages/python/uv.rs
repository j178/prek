use std::env;
use std::path::PathBuf;

use anyhow::Result;
use axoupdater::{AxoUpdater, ReleaseSource, ReleaseSourceType};
use tracing::{debug, warn};

use crate::fs::LockedFile;
use crate::store::Store;

/// Ensure that the `uv` binary is available.
pub(crate) async fn ensure_uv() -> Result<PathBuf> {
    // 1) Check if `uv` is installed already.
    if let Ok(uv) = which::which("uv") {
        return Ok(uv);
    }

    // 2) Check if `uv` is installed by `pre-commit`
    let store = Store::from_settings()?;

    let uv_dir = store.uv_path();
    let uv = uv_dir.join("uv").with_extension(env::consts::EXE_EXTENSION);
    if uv.is_file() {
        return Ok(uv);
    }

    fs_err::create_dir_all(&uv_dir)?;
    let _lock = LockedFile::acquire(uv_dir.join(".lock"), "uv").await?;

    if uv.is_file() {
        return Ok(uv);
    }

    // 3) Download and install `uv`
    let mut installer = AxoUpdater::new_for("uv");
    installer.always_update(true);
    installer.disable_installer_output();
    installer.set_install_dir(&uv_dir.to_string_lossy());

    env::set_var("AXOUPDATER_CONFIG_PATH", &uv_dir);
    if let Err(err) = installer.load_receipt() {
        warn!(err = ?err, "Failed to load receipt");
        installer.set_release_source(ReleaseSource {
            release_type: ReleaseSourceType::GitHub,
            owner: "astral-sh".to_string(),
            name: "uv".to_string(),
            app_name: "uv".to_string(),
        });
    }

    match installer.run().await {
        Ok(Some(result)) => {
            debug!(
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
