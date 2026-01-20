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
