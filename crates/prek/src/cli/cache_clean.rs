use std::fmt::Write;
use std::fs::FileType;
use std::io;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;
use tracing::error;

use crate::cli::ExitStatus;
use crate::cli::cache_size::human_readable_bytes;
use crate::cli::reporter::CleaningReporter;
use crate::printer::Printer;
use crate::store::{CacheBucket, Store};

pub(crate) fn cache_clean(store: &Store, printer: Printer) -> Result<ExitStatus> {
    if !store.path().exists() {
        writeln!(printer.stdout(), "{}", "Nothing to clean".bold())?;
        return Ok(ExitStatus::Success);
    }

    let num_paths = walkdir::WalkDir::new(store.path()).into_iter().count();
    let reporter = CleaningReporter::new(printer, num_paths);

    if let Err(e) = fix_permissions(store.cache_path(CacheBucket::Go))
        && e.kind() != io::ErrorKind::NotFound
    {
        error!("Failed to fix permissions: {}", e);
    }

    let removal = remove_dir_all(store.path(), Some(&reporter))?;

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

/// Recursively remove a directory and all its contents.
fn remove_dir_all(path: &Path, reporter: Option<&CleaningReporter>) -> io::Result<RemovalStats> {
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

    for entry in walkdir::WalkDir::new(path).contents_first(true) {
        let entry = entry?;
        if entry.file_type().is_symlink() {
            stats.num_files += 1;
            if let Ok(metadata) = entry.metadata() {
                stats.total_bytes += metadata.len();
            }
            remove_symlink(entry.path(), entry.file_type())?;
        } else if entry.file_type().is_dir() {
            stats.num_dirs += 1;
            fs_err::remove_dir_all(entry.path())?;
        } else {
            stats.num_files += 1;
            if let Ok(metadata) = entry.metadata() {
                stats.total_bytes += metadata.len();
            }
            fs_err::remove_file(entry.path())?;
        }

        reporter.map(CleaningReporter::on_clean);
    }

    reporter.map(CleaningReporter::on_complete);

    Ok(stats)
}

fn remove_symlink(path: &Path, file_type: FileType) -> io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::fs::FileTypeExt;

        if file_type.is_symlink_dir() {
            fs_err::remove_dir(path)
        } else {
            fs_err::remove_file(path)
        }
    }
    #[cfg(not(windows))]
    {
        let _ = file_type;
        fs_err::remove_file(path)
    }
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

#[cfg(test)]
mod tests {
    use super::remove_dir_all;
    use assert_fs::fixture::TempDir;

    #[test]
    fn rm_rf_counts_and_removes_tree() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(cache_root.join("nested/deep"))?;
        fs_err::write(cache_root.join("root.txt"), b"hello")?;
        fs_err::write(cache_root.join("nested/data.txt"), b"abc")?;
        fs_err::write(cache_root.join("nested/deep/end.bin"), b"zz")?;

        let stats = remove_dir_all(&cache_root, None)?;
        assert_eq!(stats.num_files, 3);
        assert_eq!(stats.num_dirs, 3);
        assert_eq!(stats.total_bytes, 10);
        assert!(!cache_root.exists());

        Ok(())
    }

    #[test]
    fn rm_rf_empty_directory() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(&cache_root)?;

        let stats = remove_dir_all(&cache_root, None)?;
        assert_eq!(stats.num_files, 0);
        assert_eq!(stats.num_dirs, 1);
        assert_eq!(stats.total_bytes, 0);
        assert!(!cache_root.exists());

        Ok(())
    }

    #[test]
    fn rm_rf_rejects_non_directory() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let file_path = temp.path().join("not-a-dir.txt");
        fs_err::write(&file_path, b"important data")?;

        let err = remove_dir_all(&file_path, None).unwrap_err();
        assert_eq!(err.kind(), std::io::ErrorKind::NotADirectory);
        assert!(file_path.exists(), "file must not be deleted");

        Ok(())
    }

    #[test]
    fn rm_rf_non_exist_directory() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let dir_path = temp.path().join("non-existent");

        let stats = remove_dir_all(&dir_path, None)?;
        assert_eq!(stats.num_files, 0);
        assert_eq!(stats.num_dirs, 0);
        assert_eq!(stats.total_bytes, 0);

        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn rm_rf_counts_symlink_entries() -> anyhow::Result<()> {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new()?;
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(&cache_root)?;

        let link_path = cache_root.join("link-to-missing");
        symlink("missing-target", &link_path)?;
        let expected_len = fs_err::symlink_metadata(&link_path)?.len();

        let stats = remove_dir_all(&cache_root, None)?;
        assert_eq!(stats.num_files, 1);
        assert_eq!(stats.num_dirs, 1);
        assert_eq!(stats.total_bytes, expected_len);
        assert!(!cache_root.exists());

        Ok(())
    }
}
