use std::fmt::Write;
use std::io;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;
use tracing::error;

use crate::cli::ExitStatus;
use crate::fs::Simplified;
use crate::printer::Printer;
use crate::store::{CacheBucket, Store};

pub(crate) fn clean(printer: Printer) -> Result<ExitStatus> {
    let store = Store::from_settings()?;

    if !store.path().exists() {
        writeln!(printer.stdout(), "Nothing to clean")?;
        return Ok(ExitStatus::Success);
    }

    if let Err(e) = fix_permissions(store.cache_path(CacheBucket::Go)) {
        error!("Failed to fix permissions: {}", e);
    }

    fs_err::remove_dir_all(store.path())?;
    writeln!(
        printer.stdout(),
        "Cleaned `{}`",
        store.path().user_display().cyan()
    )?;

    Ok(ExitStatus::Success)
}

/// Add write permission to GOMODCACHE directory recursively.
/// Go sets the permissions to read-only by default.
#[cfg(not(windows))]
pub fn fix_permissions<P: AsRef<Path>>(path: P) -> io::Result<()> {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    let path = path.as_ref();
    let metadata = fs::metadata(path)?;

    let mut permissions = metadata.permissions();
    let current_mode = permissions.mode();

    // Add write permissions for owner, group, and others
    let new_mode = current_mode | 0o222;
    permissions.set_mode(new_mode);
    fs::set_permissions(path, permissions)?;

    // If it's a directory, recursively process its contents
    if metadata.is_dir() {
        let entries = fs::read_dir(path)?;
        for entry in entries {
            let entry = entry?;
            fix_permissions(entry.path())?;
        }
    }

    Ok(())
}

#[cfg(windows)]
#[allow(clippy::unnecessary_wraps)]
pub fn fix_permissions<P: AsRef<Path>>(_path: P) -> io::Result<()> {
    // On Windows, permissions are handled differently and this function does nothing.
    Ok(())
}
