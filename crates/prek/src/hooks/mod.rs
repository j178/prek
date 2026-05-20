use std::future::Future;
use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;

use prek_consts::env_vars::EnvVars;

use crate::cli::reporter::HookRunReporter;
use crate::hook::{Hook, Repo};
pub(crate) use crate::hooks::builtin_hooks::BuiltinHooks;
pub(crate) use crate::hooks::meta_hooks::MetaHooks;
use crate::hooks::pre_commit_hooks::{PreCommitHooks, is_pre_commit_hooks};
use crate::store::Store;

mod builtin_hooks;
mod meta_hooks;
mod pre_commit_hooks;

static NO_FAST_PATH: LazyLock<bool> = LazyLock::new(|| EnvVars::is_set(EnvVars::PREK_NO_FAST_PATH));

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

pub(crate) fn may_modify_files(hook: &Hook) -> bool {
    match hook.repo() {
        Repo::Builtin { .. } => {
            BuiltinHooks::from_str(hook.id.as_str()).map_or(true, BuiltinHooks::may_modify_files)
        }
        Repo::Remote { .. } => {
            fast_path_hook(hook).is_none_or(|implemented| implemented.may_modify_files())
        }
        _ => true,
    }
}

pub async fn run_fast_path(
    _store: &Store,
    hook: &Hook,
    filenames: &[&Path],
    reporter: &HookRunReporter,
) -> anyhow::Result<(i32, Vec<u8>)> {
    let progress = reporter.on_run_start(hook, filenames.len());

    let Some(implemented) = fast_path_hook(hook) else {
        unreachable!("run_fast_path requires a supported pre-commit hook");
    };
    let result = implemented.run(hook, filenames).await;

    reporter.on_run_complete(progress);

    result
}

pub(crate) async fn run_concurrent_file_checks<'a, I, F, Fut>(
    filenames: I,
    concurrency: usize,
    check: F,
) -> anyhow::Result<(i32, Vec<u8>)>
where
    I: IntoIterator<Item = &'a Path>,
    F: Fn(&'a Path) -> Fut,
    Fut: Future<Output = anyhow::Result<(i32, Vec<u8>)>>,
{
    use futures::StreamExt;

    let mut tasks = futures::stream::iter(filenames)
        .map(check)
        .buffered(concurrency);

    let mut code = 0;
    let mut output = Vec::new();

    while let Some(result) = tasks.next().await {
        let (c, o) = result?;
        code |= c;
        output.extend(o);
    }

    Ok((code, output))
}
