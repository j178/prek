use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use clap::Parser;
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::hook::Hook;
pub(crate) use crate::hooks::builtin::BuiltinHooks;
pub(crate) use crate::hooks::meta::MetaHooks;
pub(crate) use crate::hooks::remote::remote_hook_effect;

mod builtin;
mod check_added_large_files;
mod check_case_conflict;
mod check_commit_trailers;
mod check_executables_have_shebangs;
mod check_illegal_windows_names;
mod check_json;
mod check_json5;
mod check_jsonc;
mod check_merge_conflict;
mod check_shebang_scripts_are_executable;
mod check_signed_commit;
mod check_symlinks;
mod check_toml;
mod check_vcs_permalinks;
mod check_xml;
mod check_yaml;
mod destroyed_symlinks;
mod detect_private_key;
mod file_contents_sorter;
mod fix_byte_order_marker;
mod fix_end_of_file;
mod fix_trailing_whitespace;
mod forbid_new_submodules;
mod meta;
mod mixed_line_ending;
mod no_commit_to_branch;
mod normalize_indentation;
mod pattern;
mod pretty_format_json;
mod remote;
mod replace_pattern;
mod requirements_txt_fixer;
mod shebangs;

// Erase hook implementation futures before awaiting them so dispatchers have one suspension point.
type HookFuture<'a> = Pin<Box<dyn Future<Output = anyhow::Result<HookOutput>> + 'a>>;

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
pub(crate) enum FileChanges {
    Unchanged,
    Modified,
    Unknown,
}

#[derive(Debug)]
pub(crate) struct HookOutput {
    pub(crate) exit_status: i32,
    pub(crate) output: Vec<u8>,
    pub(crate) file_changes: FileChanges,
}

impl HookOutput {
    pub(crate) fn known(exit_status: i32, output: Vec<u8>, modified: bool) -> Self {
        Self {
            exit_status,
            output,
            file_changes: if modified {
                FileChanges::Modified
            } else {
                FileChanges::Unchanged
            },
        }
    }

    pub(crate) fn unchanged(exit_status: i32, output: Vec<u8>) -> Self {
        Self::known(exit_status, output, false)
    }

    pub(crate) fn unknown(exit_status: i32, output: Vec<u8>) -> Self {
        Self {
            exit_status,
            output,
            file_changes: FileChanges::Unknown,
        }
    }

    fn merge_known(&mut self, other: Self) {
        debug_assert_ne!(self.file_changes, FileChanges::Unknown);
        debug_assert_ne!(other.file_changes, FileChanges::Unknown);

        self.exit_status |= other.exit_status;
        self.output.extend(other.output);
        if other.file_changes == FileChanges::Modified {
            self.file_changes = FileChanges::Modified;
        }
    }
}

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct FilenamesArgs {
    #[arg(value_name = "FILENAMES")]
    pub(crate) filenames: Vec<PathBuf>,
}

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct FixArgs {
    /// Report files that would change without modifying them.
    #[arg(long)]
    pub(crate) check: bool,
    #[arg(value_name = "FILENAMES")]
    pub(crate) filenames: Vec<PathBuf>,
}

pub(crate) fn contents_equal<'a>(
    mut original: &[u8],
    chunks: impl IntoIterator<Item = &'a [u8]>,
) -> bool {
    for chunk in chunks {
        let Some(rest) = original.strip_prefix(chunk) else {
            return false;
        };
        original = rest;
    }
    original.is_empty()
}

fn parse_hook_args<T: Parser>(hook: &Hook) -> anyhow::Result<T> {
    Ok(T::try_parse_from(
        hook.entry.expect_argv_entry().split_with_args(&hook.args)?,
    )?)
}

fn hook_filenames<'a>(
    configured: &'a [PathBuf],
    selected: &'a [&Path],
) -> impl Iterator<Item = &'a Path> + 'a {
    configured
        .iter()
        .map(PathBuf::as_path)
        .chain(selected.iter().copied())
}

/// Runs blocking file checks, preserving filename order in the combined output.
/// Explicit filenames finish serially before selected filenames run in parallel.
/// Each check receives the path joined to `file_base` for I/O and the original path for diagnostics.
async fn run_blocking_file_checks<F>(
    file_base: &Path,
    explicit: &[PathBuf],
    selected: &[&Path],
    check: F,
) -> anyhow::Result<HookOutput>
where
    F: Fn(&Path, &Path) -> anyhow::Result<HookOutput> + Send + Sync + 'static,
{
    let file_base = file_base.to_path_buf();
    let explicit_len = explicit.len();
    let filenames: Vec<_> = hook_filenames(explicit, selected)
        .map(Path::to_path_buf)
        .collect();
    // Enter the blocking pool once for the batch, rather than once per file or I/O operation.
    tokio::task::spawn_blocking(move || {
        let (explicit, selected) = filenames.split_at(explicit_len);
        let mut result = HookOutput::unchanged(0, Vec::new());
        for filename in explicit {
            result.merge_known(check(&file_base.join(filename), filename)?);
        }
        // Collect results in input order so diagnostics and the first I/O error are deterministic.
        let outputs: Vec<_> = selected
            .par_iter()
            .map(|filename| run_file_check(&check, &file_base, filename))
            .collect();
        for output in outputs {
            result.merge_known(output?);
        }
        Ok(result)
    })
    .await?
}

/// Runs one file check in a frame of its own, outside rayon's recursive split.
///
/// A check may keep a buffer on the stack. Inlined into the split, that buffer would sit in every
/// frame of the recursion, and a rayon worker can hold several hooks' splits on one stack.
#[inline(never)]
fn run_file_check<F>(check: &F, file_base: &Path, filename: &Path) -> anyhow::Result<HookOutput>
where
    F: Fn(&Path, &Path) -> anyhow::Result<HookOutput>,
{
    check(&file_base.join(filename), filename)
}

/// Runs per-file checks concurrently and merges their results in input order.
pub(crate) async fn run_concurrent_file_checks<'a, I, F, Fut>(
    filenames: I,
    concurrency: usize,
    check: F,
) -> anyhow::Result<HookOutput>
where
    I: IntoIterator<Item = &'a Path>,
    F: Fn(&'a Path) -> Fut,
    Fut: Future<Output = anyhow::Result<HookOutput>>,
{
    use futures_util::StreamExt;

    let mut tasks = futures_util::stream::iter(filenames)
        .map(check)
        .buffered(concurrency);

    let mut result = HookOutput::unchanged(0, Vec::new());
    while let Some(file_result) = tasks.next().await {
        result.merge_known(file_result?);
    }

    Ok(result)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::*;

    #[tokio::test]
    async fn blocking_checks_finish_explicit_duplicates_before_selected_files() -> Result<()> {
        let dir = tempfile::tempdir()?;
        fs_err::write(dir.path().join("shared"), b"0")?;
        fs_err::write(dir.path().join("other"), b"9")?;
        let explicit = [PathBuf::from("shared"), PathBuf::from("shared")];
        let selected = [Path::new("shared"), Path::new("other")];
        let result = run_blocking_file_checks(dir.path(), &explicit, &selected, |path, _| {
            let contents = fs_err::read(path)?;
            fs_err::write(path, [contents[0] + 1])?;
            Ok(HookOutput::known(1, contents, true))
        })
        .await?;

        assert_eq!(result.output, b"0129");
        assert_eq!(result.exit_status, 1);
        assert_eq!(result.file_changes, crate::hooks::FileChanges::Modified);
        assert_eq!(fs_err::read(dir.path().join("shared"))?, b"3");
        Ok(())
    }
}
