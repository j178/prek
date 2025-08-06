use std::str::FromStr;
use std::sync::LazyLock;

use constants::env_vars::EnvVars;

use crate::builtin::pre_commit_hooks::{Implemented, is_pre_commit_hooks};
use crate::hook::{Hook, Repo};

mod meta_hooks;
mod pre_commit_hooks;

static NO_FAST_PATH: LazyLock<bool> =
    LazyLock::new(|| EnvVars::is_set(EnvVars::PREFLIGIT_NO_FAST_PATH));

/// Returns true if the hook has a builtin Rust implementation.
pub fn check_fast_path(hook: &Hook) -> bool {
    match hook.repo() {
        Repo::Meta { .. } => true,
        Repo::Remote { url, .. } if is_pre_commit_hooks(url) => {
            if *NO_FAST_PATH {
                return false;
            }
            Implemented::from_str(hook.id.as_str()).is_ok()
        }
        _ => false,
    }
}

pub async fn run_fast_path(hook: &Hook, filenames: &[&String]) -> anyhow::Result<(i32, Vec<u8>)> {
    match hook.repo() {
        Repo::Meta { .. } => run_meta_hook(hook, filenames).await,
        Repo::Remote { url, .. } if is_pre_commit_hooks(url) => {
            Implemented::from_str(hook.id.as_str())
                .unwrap()
                .run(hook, filenames)
                .await
        }
        _ => unreachable!(),
    }
}

async fn run_meta_hook(hook: &Hook, filenames: &[&String]) -> anyhow::Result<(i32, Vec<u8>)> {
    match hook.id.as_str() {
        "check-hooks-apply" => meta_hooks::check_hooks_apply(hook, filenames).await,
        "check-useless-excludes" => meta_hooks::check_useless_excludes(hook, filenames).await,
        "identity" => Ok(meta_hooks::identity(hook, filenames)),
        _ => unreachable!(),
    }
}
