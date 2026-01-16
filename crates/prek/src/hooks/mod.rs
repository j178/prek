use std::future::Future;
use std::path::Path;
use std::str::FromStr;
use std::sync::LazyLock;

use prek_consts::env_vars::EnvVars;

use crate::hook::{Hook, Repo};
use crate::hooks::pre_commit_hooks::{PreCommitHooks, is_pre_commit_hooks};
use crate::store::Store;
pub(crate) use builtin_hooks::BuiltinHooks;
pub(crate) use meta_hooks::MetaHooks;

mod builtin_hooks;
mod meta_hooks;
mod pre_commit_hooks;

static NO_FAST_PATH: LazyLock<bool> = LazyLock::new(|| EnvVars::is_set(EnvVars::PREK_NO_FAST_PATH));

/// Returns true if the hook has a builtin Rust implementation.
pub fn check_fast_path(hook: &Hook) -> bool {
    if *NO_FAST_PATH {
        return false;
    }

    match hook.repo() {
        Repo::Remote { url, .. } if is_pre_commit_hooks(url) => {
            let Ok(implemented) = PreCommitHooks::from_str(hook.id.as_str()) else {
                return false;
            };
            implemented.check_supported(hook)
        }
        _ => false,
    }
}

pub async fn run_fast_path(
    _store: &Store,
    hook: &Hook,
    filenames: &[&Path],
) -> anyhow::Result<(i32, Vec<u8>)> {
    match hook.repo() {
        Repo::Remote { url, .. } if is_pre_commit_hooks(url) => {
            PreCommitHooks::from_str(hook.id.as_str())
                .unwrap()
                .run(hook, filenames)
                .await
        }
        _ => unreachable!(),
    }
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
