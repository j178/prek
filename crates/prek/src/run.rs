use std::cmp::max;
use std::ffi::OsString;
use std::path::Path;
use std::sync::LazyLock;

use anstream::ColorChoice;
use futures::{StreamExt, TryStreamExt};
use prek_consts::env_vars::EnvVars;
use tracing::trace;

use crate::hook::Hook;

pub(crate) static USE_COLOR: LazyLock<bool> =
    LazyLock::new(|| match anstream::Stderr::choice(&std::io::stderr()) {
        ColorChoice::Always | ColorChoice::AlwaysAnsi => true,
        ColorChoice::Never => false,
        // We just asked anstream for a choice, that can't be auto
        ColorChoice::Auto => unreachable!(),
    });

pub(crate) static CONCURRENCY: LazyLock<usize> = LazyLock::new(|| {
    if EnvVars::is_set(EnvVars::PREK_NO_CONCURRENCY) {
        1
    } else {
        std::thread::available_parallelism()
            .map(std::num::NonZero::get)
            .unwrap_or(1)
    }
});

fn target_concurrency(serial: bool) -> usize {
    if serial { 1 } else { *CONCURRENCY }
}

/// Iterator that yields partitions of filenames that fit within the maximum command line length.
struct Partitions<'a> {
    filenames: &'a [&'a Path],
    current_index: usize,
    command_length: usize,
    max_per_batch: usize,
    max_cli_length: usize,
}

fn platform_max_cli_length() -> usize {
    #[cfg(unix)]
    {
        let maximum = unsafe { libc::sysconf(libc::_SC_ARG_MAX) };
        let maximum = usize::try_from(maximum).expect("SC_ARG_MAX too large") - 2048;
        maximum.clamp(1 << 12, 1 << 17)
    }
    #[cfg(windows)]
    {
        (1 << 15) - 2048 // UNICODE_STRING max - headroom
    }
    #[cfg(not(any(unix, windows)))]
    {
        1 << 12
    }
}

impl<'a> Partitions<'a> {
    fn new(
        hook: &'a Hook,
        entry: &'a [String],
        filenames: &'a [&'a Path],
        concurrency: usize,
    ) -> Self {
        let max_per_batch = max(4, filenames.len().div_ceil(concurrency));
        let mut max_cli_length = platform_max_cli_length();

        let cmd = Path::new(&entry[0]);
        if cfg!(windows)
            && cmd.extension().is_some_and(|ext| {
                ext.eq_ignore_ascii_case("cmd") || ext.eq_ignore_ascii_case("bat")
            })
        {
            // Reduce max length for batch files on Windows due to cmd.exe limitations.
            // 1024 is additionally subtracted to give headroom for further
            // expansion inside the batch file.
            max_cli_length = 8192 - 1024;
        }

        if cfg!(unix) {
            // Reserve space for environment variables.
            max_cli_length = max_cli_length.saturating_sub(
                std::env::vars_os()
                    .map(|(key, value)| {
                        if key
                            .to_str()
                            .map(|key| hook.env.contains_key(key))
                            .unwrap_or(false)
                        {
                            // key is in hook.env; add it later.
                            0
                        } else {
                            key.len() + value.len() + 2 // key=value\0
                        }
                    })
                    .sum::<usize>()
                    + hook
                        .env
                        .iter()
                        .map(|(key, value)| {
                            // On UNIX, the OS string equivalent is the same length
                            key.len() + value.len() + 2 // key=value\0
                        })
                        .sum::<usize>(),
            );
        }

        let command_length = entry.iter().map(String::len).sum::<usize>()
            + entry.len()
            + hook.args.iter().map(String::len).sum::<usize>()
            + hook.args.len();

        Self {
            filenames,
            current_index: 0,
            command_length,
            max_per_batch,
            max_cli_length,
        }
    }
}

impl<'a> Iterator for Partitions<'a> {
    type Item = &'a [&'a Path];

    fn next(&mut self) -> Option<Self::Item> {
        // Handle empty filenames case
        if self.filenames.is_empty() && self.current_index == 0 {
            self.current_index = 1;
            return Some(&[]);
        }

        if self.current_index >= self.filenames.len() {
            return None;
        }

        let start_index = self.current_index;
        let mut current_length = self.command_length + 1;

        while self.current_index < self.filenames.len() {
            let filename = self.filenames[self.current_index];
            let length = filename.as_os_str().len() + 1;

            if current_length + length > self.max_cli_length
                || self.current_index - start_index >= self.max_per_batch
            {
                break;
            }

            current_length += length;
            self.current_index += 1;
        }

        if self.current_index == start_index {
            // If we couldn't add even a single file to this batch, it means the file
            // is too long to fit in the command line by itself.
            let filename = self.filenames[self.current_index];
            let filename_length = filename.as_os_str().len() + 1;
            let filename_display = filename.to_string_lossy();
            let filename_display = format!(
                "{}...{}",
                &filename_display[..10],
                &filename_display[filename_display.len().saturating_sub(10)..]
            );
            panic!(
                "Filename `{filename_display}` ({filename_length} bytes) is too long to fit in command line",
            );
        } else {
            Some(&self.filenames[start_index..self.current_index])
        }
    }
}

pub(crate) async fn run_by_batch<T, F>(
    hook: &Hook,
    filenames: &[&Path],
    entry: &[String],
    run: F,
) -> anyhow::Result<Vec<T>>
where
    F: for<'a> AsyncFn(&'a [&'a Path]) -> anyhow::Result<T>,
    T: Send + 'static,
{
    let concurrency = target_concurrency(hook.require_serial);

    // Split files into batches
    let partitions = Partitions::new(hook, entry, filenames, concurrency);
    trace!(
        total_files = filenames.len(),
        concurrency = concurrency,
        "Running {}",
        hook.id,
    );

    #[allow(clippy::redundant_closure)]
    let results: Vec<_> = futures::stream::iter(partitions)
        .map(|batch| run(batch))
        .buffered(concurrency)
        .try_collect()
        .await?;

    Ok(results)
}

pub(crate) fn prepend_paths(paths: &[&Path]) -> Result<OsString, std::env::JoinPathsError> {
    std::env::join_paths(
        paths.iter().map(|p| p.to_path_buf()).chain(
            EnvVars::var_os(EnvVars::PATH)
                .as_ref()
                .iter()
                .flat_map(std::env::split_paths),
        ),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    /// Helper to create a Partitions iterator for testing.
    /// This bypasses the Hook requirement by directly constructing the struct.
    fn create_test_partitions<'a>(
        filenames: &'a [&'a Path],
        command_length: usize,
        max_cli_length: usize,
        max_per_batch: usize,
    ) -> Partitions<'a> {
        Partitions {
            filenames,
            current_index: 0,
            command_length,
            max_per_batch,
            max_cli_length,
        }
    }

    #[test]
    fn test_partitions_normal_filenames() {
        let file1 = PathBuf::from("file1.txt");
        let file2 = PathBuf::from("file2.txt");
        let file3 = PathBuf::from("file3.txt");
        let filenames: Vec<&Path> = vec![&file1, &file2, &file3];

        let partitions = create_test_partitions(&filenames, 100, 4096, 10);

        let total_files: usize = partitions.map(<[&Path]>::len).sum();

        // All files should have been processed (no panic)
        assert_eq!(total_files, 3);
    }

    #[test]
    fn test_partitions_empty_filenames() {
        let filenames: Vec<&Path> = vec![];

        let mut partitions = create_test_partitions(&filenames, 100, 4096, 10);

        // Should return empty slice once, then None
        let batch = partitions.next();
        assert!(batch.is_some());
        assert_eq!(batch.unwrap().len(), 0);

        let batch = partitions.next();
        assert!(batch.is_none());
    }

    #[test]
    #[should_panic(expected = "is too long")]
    fn test_partitions_long_filename_in_middle_panics() {
        let file1 = PathBuf::from("file1.txt");
        let long_name = "a".repeat(5000);
        let long_file = PathBuf::from(&long_name);
        let file3 = PathBuf::from("file3.txt");
        let filenames: Vec<&Path> = vec![&file1, &long_file, &file3];

        let mut partitions = create_test_partitions(&filenames, 100, 1000, 10);

        // First batch should succeed with file1
        let batch1 = partitions.next();
        assert!(batch1.is_some());

        // Second batch should panic on the long filename
        // This ensures we don't silently skip file3
        partitions.next();
    }

    #[test]
    fn test_partitions_respects_max_per_batch() {
        // Create many small files
        let files: Vec<PathBuf> = (0..100)
            .map(|i| PathBuf::from(format!("f{i}.txt")))
            .collect();
        let file_refs: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();

        let partitions = create_test_partitions(&file_refs, 100, 100_000, 25);

        let all_batches: Vec<_> = partitions.map(<[&Path]>::len).collect();

        // Should have multiple batches due to max_per_batch
        assert!(all_batches.len() >= 4);

        // All files should have been processed
        let total_files: usize = all_batches.iter().sum();
        assert_eq!(total_files, 100);
    }

    #[test]
    fn test_partitions_respects_cli_length_limit() {
        // Create files that will exceed CLI length limit
        let files: Vec<PathBuf> = (0..10)
            .map(|i| PathBuf::from(format!("file{i}.txt")))
            .collect();
        let file_refs: Vec<&Path> = files.iter().map(PathBuf::as_path).collect();

        // Set a small max_cli_length to force multiple batches
        let partitions = create_test_partitions(&file_refs, 50, 150, 100);

        let all_batches: Vec<_> = partitions.map(<[&Path]>::len).collect();

        // Should have multiple batches due to CLI length limit
        assert!(all_batches.len() > 1);

        // All files should have been processed
        let total_files: usize = all_batches.iter().sum();
        assert_eq!(total_files, 10);
    }
}
