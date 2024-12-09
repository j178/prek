use crate::hook::{Hook, Repo};
use std::collections::HashMap;
use std::sync::Arc;

mod meta_hooks;

/// Returns true if the hook has a builtin Rust implementation.
pub fn check_fast_path(hook: &Hook) -> bool {
    if matches!(hook.repo(), Repo::Meta { .. }) {
        return true;
    };

    false
}

pub async fn run_fast_path(
    hook: &Hook,
    filenames: &[&String],
    env_vars: Arc<HashMap<&'static str, String>>,
) -> anyhow::Result<(i32, Vec<u8>)> {
    match hook.repo() {
        Repo::Meta { .. } => run_meta_hook(hook, filenames, env_vars).await,
        _ => unreachable!(),
    }
}

async fn run_meta_hook(
    hook: &Hook,
    filenames: &[&String],
    env_vars: Arc<HashMap<&'static str, String>>,
) -> anyhow::Result<(i32, Vec<u8>)> {
    match hook.id.as_str() {
        "check-hooks-apply" => meta_hooks::check_hooks_apply(hook, filenames, env_vars).await,
        "check-useless-excludes" => {
            meta_hooks::check_useless_excludes(hook, filenames, env_vars).await
        }
        "identity" => Ok(meta_hooks::identity(hook, filenames, env_vars)),
        _ => unreachable!(),
    }
}
