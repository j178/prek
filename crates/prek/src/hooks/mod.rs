use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::str::FromStr;
use std::sync::LazyLock;

use clap::Parser;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use rayon::iter::{IntoParallelRefIterator, ParallelIterator};

use crate::cli::run::HookRunReporter;
use crate::hook::{Hook, Repo};
pub(crate) use crate::hooks::builtin_hooks::BuiltinHooks;
pub(crate) use crate::hooks::meta_hooks::MetaHooks;
use crate::hooks::pre_commit_hooks::{PreCommitHooks, is_pre_commit_hooks};
use crate::store::Store;

mod builtin_hooks;
mod meta_hooks;
mod pre_commit_hooks;

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

static NO_FAST_PATH: LazyLock<bool> = LazyLock::new(|| EnvVars.is_set(EnvVars::PREK_NO_FAST_PATH));

/// Returns true if the hook has a builtin Rust implementation.
pub fn check_fast_path(hook: &Hook) -> bool {
    fast_path_hook(hook).is_some()
}

fn fast_path_hook(hook: &Hook) -> Option<PreCommitHooks> {
    // TODO: Decide whether fast-path hooks should honor or ignore a configured `shell`.
    if *NO_FAST_PATH || hook.language_overridden {
        return None;
    }

    let Repo::Remote { url, .. } = hook.repo() else {
        return None;
    };
    if !is_pre_commit_hooks(url) {
        return None;
    }

    PreCommitHooks::from_str(hook.id.as_str()).ok()
}

/// Returns whether a hook requires a Git diff to determine file changes.
pub(crate) fn requires_diff_tracking(hook: &Hook) -> bool {
    match hook.repo() {
        Repo::Meta | Repo::Builtin => false,
        Repo::Remote { .. } => fast_path_hook(hook).is_none() && hook.language.can_modify_files(),
        Repo::Local => hook.language.can_modify_files(),
    }
}

pub(crate) async fn run_fast_path(
    _store: &Store,
    hook: &Hook,
    filenames: &[&Path],
    reporter: &HookRunReporter,
) -> anyhow::Result<HookOutput> {
    let progress = reporter.on_run_start(hook, filenames.len());

    let implemented =
        fast_path_hook(hook).expect("run_fast_path requires a supported pre-commit hook");
    let result = implemented.run(hook, filenames).await;

    reporter.on_run_complete(progress);

    result
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
            .map(|filename| check(&file_base.join(filename), filename))
            .collect();
        for output in outputs {
            result.merge_known(output?);
        }
        Ok(result)
    })
    .await?
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
