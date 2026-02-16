use std::fmt::Write;
use std::path::Path;

use anyhow::Result;

use crate::cli::ExitStatus;
use crate::printer::Printer;
use crate::store::Store;

/// Display the total size of the cache.
pub(crate) fn cache_size(
    store: &Store,
    human_readable: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    // Walk the entire cache root
    let total_bytes = dir_size_bytes(store.path());
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

    let bytes_f32 = bytes as f32;
    let i = ((bytes_f32.log2() / 10.0) as usize).min(UNITS.len() - 1);
    (bytes_f32 / 1024_f32.powi(i as i32), UNITS[i])
}

pub(crate) fn dir_size_bytes(path: &Path) -> u64 {
    if !path.exists() {
        return 0;
    }

    walkdir::WalkDir::new(path)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter_map(|entry| match entry.metadata() {
            Ok(metadata) if metadata.is_file() => Some(metadata.len()),
            _ => None,
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::{dir_size_bytes, human_readable_bytes};
    use assert_fs::fixture::TempDir;

    #[test]
    fn human_readable_bytes_handles_zero() {
        let (value, unit) = human_readable_bytes(0);
        assert!(value.abs() < f32::EPSILON);
        assert_eq!(unit, "B");
    }

    #[test]
    fn dir_stats_missing_directory() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let missing = temp.path().join("missing");

        assert_eq!(dir_size_bytes(&missing), 0);

        Ok(())
    }

    #[test]
    fn dir_stats_empty_directory() -> anyhow::Result<()> {
        let temp = TempDir::new()?;

        assert_eq!(dir_size_bytes(temp.path()), 0);

        Ok(())
    }

    #[test]
    fn dir_stats_nested_files() -> anyhow::Result<()> {
        let temp = TempDir::new()?;
        let nested = temp.path().join("nested/deep");
        fs_err::create_dir_all(&nested)?;
        fs_err::write(temp.path().join("root.txt"), b"hello")?;
        fs_err::write(temp.path().join("nested/data.txt"), b"abc")?;
        fs_err::write(temp.path().join("nested/deep/end.bin"), b"zz")?;

        assert_eq!(dir_size_bytes(temp.path()), 10);

        Ok(())
    }
}
