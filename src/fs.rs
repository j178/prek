// MIT License
//
// Copyright (c) 2023 Astral Software Inc.
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in all
// copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
// SOFTWARE.

use std::fmt::Display;
use std::path::Path;

use tracing::{debug, error, info, trace};

use anyhow::Context;

/// A file lock that is automatically released when dropped.
#[derive(Debug)]
pub struct LockedFile(fs_err::File);

impl LockedFile {
    /// Inner implementation for [`LockedFile::acquire_blocking`] and [`LockedFile::acquire`].
    fn lock_file_blocking(file: fs_err::File, resource: &str) -> Result<Self, std::io::Error> {
        trace!(
            resource,
            path = %file.path().display(),
            "Checking lock",
        );
        match file.try_lock() {
            Ok(()) => {
                debug!(resource, "Acquired lock");
                Ok(Self(file))
            }
            Err(err) => {
                // Log error code and enum kind to help debugging more exotic failures
                if !matches!(err, std::fs::TryLockError::WouldBlock) {
                    trace!(error = ?err, "Try lock error");
                }
                info!(
                    resource,
                    path = %file.path().display(),
                    "Waiting to acquire lock",
                );
                file.lock().map_err(|err| {
                    // Not a fs_err method, we need to build our own path context
                    std::io::Error::other(format!(
                        "Could not acquire lock for `{resource}` at `{}`: {}",
                        file.path().display(),
                        err
                    ))
                })?;

                trace!(resource, "Acquired lock");
                Ok(Self(file))
            }
        }
    }

    /// Acquire a cross-process lock for a resource using a file at the provided path.
    pub async fn acquire(
        path: impl AsRef<Path>,
        resource: impl Display,
    ) -> Result<Self, std::io::Error> {
        let file = fs_err::File::create(path.as_ref())?;
        let resource = resource.to_string();
        tokio::task::spawn_blocking(move || Self::lock_file_blocking(file, &resource)).await?
    }
}

impl Drop for LockedFile {
    fn drop(&mut self) {
        if let Err(err) = self.0.file().unlock() {
            error!(
                "Failed to unlock {}; program may be stuck: {}",
                self.0.path().display(),
                err
            );
        } else {
            trace!(path = %self.0.path().display(), "Released lock");
        }
    }
}

/// Create a symlink or copy the file on Windows.
/// Tries symlink first, falls back to copy if symlink fails.
pub(crate) async fn create_symlink_or_copy(source: &Path, target: &Path) -> anyhow::Result<()> {
    if target.exists() {
        fs_err::tokio::remove_file(target).await?;
    }

    #[cfg(not(windows))]
    {
        // Try symlink on Unix systems
        match fs_err::tokio::symlink(source, target).await {
            Ok(()) => {
                trace!(
                    "Created symlink from `{}` to `{}`",
                    source.display(),
                    target.display()
                );
                return Ok(());
            }
            Err(e) => {
                trace!(
                    "Failed to create symlink from `{}` to `{}`: {}",
                    source.display(),
                    target.display(),
                    e
                );
            }
        }
    }

    #[cfg(windows)]
    {
        // Try Windows symlink API (requires admin privileges)
        use std::os::windows::fs::symlink_file;
        match symlink_file(source, target) {
            Ok(()) => {
                trace!(
                    "Created Windows symlink from {} to {}",
                    source.display(),
                    target.display()
                );
                return Ok(());
            }
            Err(e) => {
                trace!(
                    "Failed to create Windows symlink from {} to {}: {}",
                    source.display(),
                    target.display(),
                    e
                );
            }
        }
    }

    // Fallback to copy
    trace!(
        "Falling back to copy from `{}` to `{}`",
        source.display(),
        target.display()
    );
    fs_err::tokio::copy(source, target).await.with_context(|| {
        format!(
            "Failed to copy file from {} to {}",
            source.display(),
            target.display(),
        )
    })?;

    Ok(())
}

pub(crate) async fn rename_or_copy(source: &Path, target: &Path) -> std::io::Result<()> {
    // Try to rename first
    match fs_err::tokio::rename(source, target).await {
        Ok(()) => {
            trace!("Renamed `{}` to `{}`", source.display(), target.display());
            Ok(())
        }
        Err(e) if e.kind() == std::io::ErrorKind::CrossesDevices => {
            trace!(
                "Falling back to copy from `{}` to `{}`",
                source.display(),
                target.display()
            );
            fs_err::tokio::copy(source, target).await?;
            fs_err::tokio::remove_file(source).await?;
            Ok(())
        }
        Err(e) => {
            trace!(
                "Failed to rename `{}` to `{}`: {}",
                source.display(),
                target.display(),
                e
            );
            Err(e)
        }
    }
}
