use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::LazyLock;

use prek_consts::env_vars::{EnvVars, EnvVarsRead};

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
    if *NO_FAST_PATH {
        return None;
    }

    let Repo::Remote { url, .. } = hook.repo() else {
        return None;
    };
    if !is_pre_commit_hooks(url) {
        return None;
    }

    let implemented = PreCommitHooks::from_str(hook.id.as_str()).ok()?;
    if implemented.check_supported(hook) {
        Some(implemented)
    } else {
        None
    }
}

/// Returns whether a hook requires a Git diff to determine file changes.
pub(crate) fn requires_diff_tracking(hook: &Hook) -> bool {
    match hook.repo() {
        Repo::Meta { .. } | Repo::Builtin { .. } => false,
        Repo::Remote { .. } => fast_path_hook(hook).is_none() && hook.language.can_modify_files(),
        Repo::Local { .. } => hook.language.can_modify_files(),
    }
}

pub async fn run_fast_path(
    _store: &Store,
    hook: &Hook,
    filenames: &[&Path],
    reporter: &HookRunReporter,
) -> anyhow::Result<HookOutput> {
    let progress = reporter.on_run_start(hook, filenames.len());

    let Some(implemented) = fast_path_hook(hook) else {
        unreachable!("run_fast_path requires a supported pre-commit hook");
    };
    let result = implemented.run(hook, filenames).await;

    reporter.on_run_complete(progress);

    result
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
