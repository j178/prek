use std::fmt::Write;
use std::path::Path;

use anyhow::Result;

use crate::cli::ExitStatus;
use crate::printer::Printer;
use crate::store::Store;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct DirStats {
    pub(crate) file_count: u64,
    pub(crate) total_bytes: u64,
}

/// Display the total size of the cache.
pub(crate) fn cache_size(
    store: &Store,
    human_readable: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    // Walk the entire cache root
    let total_bytes = dir_stats(store.path())?.total_bytes;
    if human_readable {
        let (bytes, unit) = human_readable_bytes(total_bytes);
        writeln!(printer.stdout_important(), "{bytes:.1}{unit}")?;
    } else {
        writeln!(printer.stdout_important(), "{total_bytes}")?;
    }

    Ok(ExitStatus::Success)
}

/// Formats a number of bytes into a human readable SI-prefixed size (binary units).
///
/// Returns a tuple of `(quantity, units)`.
#[allow(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]
pub(crate) fn human_readable_bytes(bytes: u64) -> (f32, &'static str) {
    const UNITS: [&str; 7] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB", "EiB"];
    if bytes == 0 {
        return (0.0, UNITS[0]);
    }

    let bytes_f32 = bytes as f32;
    let i = ((bytes_f32.log2() / 10.0) as usize).min(UNITS.len() - 1);
    (bytes_f32 / 1024_f32.powi(i as i32), UNITS[i])
}

pub(crate) fn dir_stats(path: &Path) -> Result<DirStats> {
    if !path.exists() {
        return Ok(DirStats::default());
    }

    walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .try_fold(DirStats::default(), |mut stats, entry| {
            let entry = entry?;
            let metadata = entry.metadata()?;
            if metadata.is_file() {
                stats.file_count = stats.file_count.saturating_add(1);
                stats.total_bytes = stats.total_bytes.saturating_add(metadata.len());
            }
            Ok(stats)
        })
}

pub(crate) fn dir_size_bytes(path: &Path) -> u64 {
    dir_stats(path)
        .map(|stats| stats.total_bytes)
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::{DirStats, dir_stats, human_readable_bytes};
    use assert_fs::fixture::TempDir;

    #[test]
    fn human_readable_bytes_handles_zero() {
        let (value, unit) = human_readable_bytes(0);
        assert!(value.abs() < f32::EPSILON);
        assert_eq!(unit, "B");
    }

    #[test]
    fn dir_stats_missing_directory() {
        let temp = TempDir::new().expect("create temp dir");
        let missing = temp.path().join("missing");

        assert_eq!(
            dir_stats(&missing).expect("missing dir stats"),
            DirStats::default()
        );
    }

    #[test]
    fn dir_stats_empty_directory() {
        let temp = TempDir::new().expect("create temp dir");

        assert_eq!(
            dir_stats(temp.path()).expect("empty dir stats"),
            DirStats::default()
        );
    }

    #[test]
    fn dir_stats_nested_files() {
        let temp = TempDir::new().expect("create temp dir");
        let nested = temp.path().join("nested/deep");
        fs_err::create_dir_all(&nested).expect("create nested dirs");
        fs_err::write(temp.path().join("root.txt"), b"hello").expect("write root file");
        fs_err::write(temp.path().join("nested/data.txt"), b"abc").expect("write nested file");
        fs_err::write(temp.path().join("nested/deep/end.bin"), b"zz").expect("write deep file");

        assert_eq!(
            dir_stats(temp.path()).expect("nested dir stats"),
            DirStats {
                file_count: 3,
                total_bytes: 10,
            }
        );
    }
}
