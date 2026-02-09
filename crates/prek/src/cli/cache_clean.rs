use std::fmt::Write;
use std::io;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;
use tracing::error;

use crate::cli::ExitStatus;
use crate::cli::cache_size::{DirStats, human_readable_bytes};
use crate::printer::Printer;
use crate::store::{CacheBucket, Store};

pub(crate) fn cache_clean(store: &Store, printer: Printer) -> Result<ExitStatus> {
    if !store.path().exists() {
        writeln!(printer.stdout(), "Nothing to clean")?;
        return Ok(ExitStatus::Success);
    }

    if let Err(e) = fix_permissions(store.cache_path(CacheBucket::Go))
        && e.kind() != io::ErrorKind::NotFound
    {
        error!("Failed to fix permissions: {}", e);
    }

    let stats = remove_dir_all_with_stats(store.path())?;
    writeln!(
        printer.stdout(),
        "Cleaned `{}`",
        store.path().display().cyan()
    )?;
    if stats.file_count > 0 {
        let (size, unit) = human_readable_bytes(stats.total_bytes);
        writeln!(
            printer.stdout(),
            "Removed {} ({}{unit})",
            file_label(stats.file_count),
            format!("{size:.1}").cyan().bold(),
        )?;
    }

    Ok(ExitStatus::Success)
}

fn file_label(file_count: u64) -> String {
    if file_count == 1 {
        "1 file".to_string()
    } else {
        format!("{file_count} files")
    }
}

/// Recursively removes a directory and all its contents, returning aggregate
/// file count and byte totals. Returns an error if `path` is not a directory.
fn remove_dir_all_with_stats(path: &Path) -> io::Result<DirStats> {
    let metadata = fs_err::symlink_metadata(path)?;
    if !metadata.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            format!("not a directory: {}", path.display()),
        ));
    }

    let mut stats = DirStats::default();
    for entry in fs_err::read_dir(path)? {
        let entry = entry?;
        let entry_path = entry.path();
        let entry_stats = remove_entry_with_stats(&entry_path)?;
        stats.file_count = stats.file_count.saturating_add(entry_stats.file_count);
        stats.total_bytes = stats.total_bytes.saturating_add(entry_stats.total_bytes);
    }

    fs_err::remove_dir(path)?;
    Ok(stats)
}

fn remove_entry_with_stats(path: &Path) -> io::Result<DirStats> {
    let metadata = fs_err::symlink_metadata(path)?;
    if metadata.is_dir() {
        return remove_dir_all_with_stats(path);
    }

    remove_file_with_stats(path, &metadata)
}

fn remove_file_with_stats(path: &Path, metadata: &std::fs::Metadata) -> io::Result<DirStats> {
    let mut stats = DirStats::default();
    if metadata.is_file() || metadata.file_type().is_symlink() {
        stats.file_count = 1;
        stats.total_bytes = metadata.len();
    }

    fs_err::remove_file(path)?;
    Ok(stats)
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
    use super::{DirStats, file_label, remove_dir_all_with_stats};
    use assert_fs::fixture::TempDir;

    #[test]
    fn file_label_uses_singular_and_plural() {
        assert_eq!(file_label(0), "0 files");
        assert_eq!(file_label(1), "1 file");
        assert_eq!(file_label(2), "2 files");
    }

    #[test]
    fn remove_dir_all_with_stats_counts_and_removes_tree() {
        let temp = TempDir::new().expect("create temp dir");
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(cache_root.join("nested/deep")).expect("create nested dirs");
        fs_err::write(cache_root.join("root.txt"), b"hello").expect("write root file");
        fs_err::write(cache_root.join("nested/data.txt"), b"abc").expect("write nested file");
        fs_err::write(cache_root.join("nested/deep/end.bin"), b"zz").expect("write deep file");

        let stats = remove_dir_all_with_stats(&cache_root).expect("remove dir with stats");
        assert_eq!(stats.file_count, 3);
        assert_eq!(stats.total_bytes, 10);
        assert!(!cache_root.exists());
    }

    #[test]
    fn remove_dir_all_with_stats_empty_directory() {
        let temp = TempDir::new().expect("create temp dir");
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(&cache_root).expect("create cache dir");

        let stats = remove_dir_all_with_stats(&cache_root).expect("remove empty dir with stats");
        assert_eq!(stats, DirStats::default());
        assert!(!cache_root.exists());
    }

    #[test]
    fn remove_dir_all_with_stats_rejects_non_directory() {
        let temp = TempDir::new().expect("create temp dir");
        let file_path = temp.path().join("not-a-dir.txt");
        fs_err::write(&file_path, b"important data").expect("write file");

        let err = remove_dir_all_with_stats(&file_path).expect_err("should reject non-directory");
        assert_eq!(err.kind(), std::io::ErrorKind::NotADirectory);
        assert!(file_path.exists(), "file must not be deleted");
    }

    #[cfg(unix)]
    #[test]
    fn remove_dir_all_with_stats_counts_symlink_entries() {
        use std::os::unix::fs::symlink;

        let temp = TempDir::new().expect("create temp dir");
        let cache_root = temp.path().join("cache");
        fs_err::create_dir_all(&cache_root).expect("create cache dir");

        let link_path = cache_root.join("link-to-missing");
        symlink("missing-target", &link_path).expect("create symlink");
        let expected_len = fs_err::symlink_metadata(&link_path)
            .expect("symlink metadata")
            .len();

        let stats = remove_dir_all_with_stats(&cache_root).expect("remove dir with symlink");
        assert_eq!(stats.file_count, 1);
        assert_eq!(stats.total_bytes, expected_len);
        assert!(!cache_root.exists());
    }
}
