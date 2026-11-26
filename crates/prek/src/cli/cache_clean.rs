use std::fmt::Write;
use std::io;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::ExitStatus;
use crate::cli::cache_size::human_readable_bytes;
use crate::cli::reporter::CleaningReporter;
use crate::printer::Printer;
use crate::store::{LockedStore, STORE_LOCK};

pub(crate) fn cache_clean(store: &LockedStore<'_>, printer: Printer) -> Result<ExitStatus> {
    let reporter = if printer.target().is_hidden() {
        None
    } else {
        Some(CleaningReporter::new(
            printer,
            cache_entries(store.path()).count(),
        ))
    };

    let removal = clear_cache(store.path(), reporter.as_ref())?;

    match (removal.num_files, removal.num_dirs) {
        (0, 0) => {
            write!(printer.stderr(), "No cache entries found")?;
        }
        (0, 1) => {
            write!(printer.stderr(), "Removed 1 directory")?;
        }
        (0, num_dirs_removed) => {
            write!(printer.stderr(), "Removed {num_dirs_removed} directories")?;
        }
        (1, _) => {
            write!(printer.stderr(), "Removed 1 file")?;
        }
        (num_files_removed, _) => {
            write!(printer.stderr(), "Removed {num_files_removed} files")?;
        }
    }

    // If any, write a summary of the total byte count removed.
    if removal.total_bytes > 0 {
        let (bytes, unit) = human_readable_bytes(removal.total_bytes);
        let bytes = format!("{bytes:.1}{unit}");
        write!(printer.stderr(), " ({})", bytes.cyan().bold())?;
    }

    writeln!(printer.stderr())?;

    Ok(ExitStatus::Success)
}

#[derive(Debug, Default)]
pub struct RemovalStats {
    pub num_files: u64,
    pub num_dirs: u64,
    pub total_bytes: u64,
}

// All processes must open the same lock file, including those that arrive during cleanup.
fn cache_entries(root: &Path) -> impl Iterator<Item = walkdir::Result<walkdir::DirEntry>> {
    walkdir::WalkDir::new(root)
        .follow_root_links(false)
        .min_depth(1)
        .contents_first(true)
        .into_iter()
        .filter(|entry| !matches!(entry, Ok(e) if e.depth() == 1 && e.file_name() == STORE_LOCK))
}

fn clear_cache(path: &Path, reporter: Option<&CleaningReporter>) -> io::Result<RemovalStats> {
    match fs_err::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() {
                return Err(io::Error::new(
                    io::ErrorKind::NotADirectory,
                    format!(
                        "Expected a directory at {}, but found a file",
                        path.display()
                    ),
                ));
            }
        }
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(RemovalStats::default()),
        Err(err) => return Err(err),
    }

    let mut stats = RemovalStats::default();

    for entry in cache_entries(path) {
        let entry = entry?;
        if entry.file_type().is_dir() {
            stats.num_dirs += 1;
        } else {
            stats.num_files += 1;
            if let Ok(metadata) = entry.metadata() {
                stats.total_bytes += metadata.len();
            }
        }
        remove_entry(&entry)?;
        reporter.map(CleaningReporter::on_clean);
    }

    reporter.map(CleaningReporter::on_complete);

    Ok(stats)
}

fn remove_entry(entry: &walkdir::DirEntry) -> io::Result<()> {
    let path = entry.path();
    let is_dir = entry.file_type().is_dir();
    #[cfg(windows)]
    let is_dir = {
        use std::os::windows::fs::FileTypeExt;
        is_dir || entry.file_type().is_symlink_dir()
    };
    let remove = || {
        if is_dir {
            fs_err::remove_dir(path)
        } else {
            fs_err::remove_file(path)
        }
    };
    let result = remove();
    // Go modules can contain read-only directories. Unlinking requires write access
    // to the parent, even when the entry itself is a symlink or a read-only file.
    #[cfg(unix)]
    if let Err(err) = &result
        && err.kind() == io::ErrorKind::PermissionDenied
        && entry.depth() > 1
        && let Some(parent) = path.parent()
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs_err::symlink_metadata(parent)?.permissions();
        if permissions.mode() & 0o200 == 0 {
            permissions.set_mode(permissions.mode() | 0o200);
            fs_err::set_permissions(parent, permissions)?;
            return remove();
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{STORE_LOCK, clear_cache};
    use assert_fs::fixture::TempDir;

    #[test]
    fn clear_cache_preserves_lock_and_counts_removed_entries() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(cache_root.join("nested/deep"))?;
        fs_err::write(cache_root.join(STORE_LOCK), b"")?;
        fs_err::write(cache_root.join("root.txt"), b"hello")?;
        fs_err::write(cache_root.join("nested").join(STORE_LOCK), b"abc")?;
        fs_err::write(cache_root.join("nested/deep/end.bin"), b"zz")?;

        let stats = clear_cache(&cache_root, None)?;
        assert_eq!(stats.num_files, 3);
        assert_eq!(stats.num_dirs, 2);
        assert_eq!(stats.total_bytes, 10);
        assert!(cache_root.join(STORE_LOCK).is_file());
        assert_eq!(fs_err::read_dir(&cache_root)?.count(), 1);

        Ok(())
    }

    #[test]
    fn clear_cache_empty_directory() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(&cache_root)?;

        let stats = clear_cache(&cache_root, None)?;
        assert_eq!(stats.num_files, 0);
        assert_eq!(stats.num_dirs, 0);
        assert_eq!(stats.total_bytes, 0);
        assert_eq!(fs_err::read_dir(&cache_root)?.count(), 0);

        Ok(())
    }

    #[test]
    fn clear_cache_rejects_non_directory() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let file_path = temp.path().join("not-a-dir.txt");
        fs_err::write(&file_path, b"important data")?;

        let err = clear_cache(&file_path, None).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotADirectory);
        assert!(file_path.exists(), "file must not be deleted");

        Ok(())
    }

    #[test]
    fn clear_cache_non_exist_directory() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let dir_path = temp.path().join("non-existent");

        let stats = clear_cache(&dir_path, None)?;
        assert_eq!(stats.num_files, 0);
        assert_eq!(stats.num_dirs, 0);
        assert_eq!(stats.total_bytes, 0);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn clear_cache_counts_symlink_entries() -> anyhow::Result<()> {
        use fs_err::os::unix::fs::symlink;

        let temp = TempDir::new()?;
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(&cache_root)?;

        let link_path = cache_root.join("link-to-missing");
        symlink("missing-target", &link_path)?;
        let expected_len = fs_err::symlink_metadata(&link_path)?.len();

        let stats = clear_cache(&cache_root, None)?;
        assert_eq!(stats.num_files, 1);
        assert_eq!(stats.num_dirs, 0);
        assert_eq!(stats.total_bytes, expected_len);
        assert_eq!(fs_err::read_dir(&cache_root)?.count(), 0);

        Ok(())
    }
}
