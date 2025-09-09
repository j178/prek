use std::borrow::Cow;
use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use rustc_hash::FxHashSet;
use tracing::{debug, warn};

use crate::cli::ExitStatus;
use crate::config::{Language, Repo as ConfigRepo, load_config};
use crate::hook::{Hook, HookBuilder, HookSpec, Repo as HookRepo};
use crate::printer::Printer;
use crate::store::{CacheBucket, Store, ToolBucket};
use crate::workspace::Project;

pub(crate) async fn cache_gc(store: &Store, printer: Printer) -> Result<ExitStatus> {
    let _lock = store.lock_async().await?;

    let tracked_configs = store.tracked_configs()?;
    if tracked_configs.is_empty() {
        writeln!(printer.stdout(), "Nothing to clean")?;
        return Ok(ExitStatus::Success);
    }

    let mut kept_configs = FxHashSet::default();
    let mut used_repo_keys: FxHashSet<String> = FxHashSet::default();
    let mut used_hook_env_dirs: FxHashSet<String> = FxHashSet::default();
    let mut used_tools: FxHashSet<ToolBucket> = FxHashSet::default();
    let mut used_cache: FxHashSet<CacheBucket> = FxHashSet::default();

    let installed = store.installed_hooks().await;

    for config_path in &tracked_configs {
        if !config_path.is_file() {
            debug!(path = %config_path.display(), "Tracked config does not exist, dropping");
            continue;
        }

        // Ensure the file is parseable; if not, keep tracking but skip marking.
        if let Err(err) = load_config(config_path) {
            warn!(path = %config_path.display(), %err, "Failed to parse config, skipping for GC");
            kept_configs.insert(config_path.clone());
            continue;
        }
        kept_configs.insert(config_path.clone());

        let mut hooks = match build_hooks_from_config(store, config_path).await {
            Ok(hooks) => hooks,
            Err(err) => {
                warn!(path = %config_path.display(), %err, "Failed to build hooks from config, skipping hook/env marking");
                Vec::new()
            }
        };

        // Mark repos referenced by this config (if present in store).
        // We do this via config parsing (no clone), so GC won't keep repos for missing configs.
        if let Ok(config) = load_config(config_path) {
            for repo in &config.repos {
                if let ConfigRepo::Remote(remote) = repo {
                    let key = Store::repo_key(remote);
                    used_repo_keys.insert(key);
                }
            }
        }

        // Mark tools/caches and hook environments by matching already-installed envs.
        for hook in &mut hooks {
            mark_tools_and_cache_for_hook(hook, &mut used_tools, &mut used_cache);

            for info in &installed {
                if info.matches(hook) {
                    if let Some(dir) = info
                        .env_path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .map(str::to_string)
                    {
                        used_hook_env_dirs.insert(dir);
                    }
                    break;
                }
            }
        }
    }

    // Update tracking file to drop configs that no longer exist.
    if kept_configs.len() != tracked_configs.len() {
        store.update_tracked_configs(&kept_configs)?;
    }

    let mut removed_repos = 0usize;
    let mut removed_hooks = 0usize;
    let mut removed_tools = 0usize;
    let mut removed_cache = 0usize;

    removed_repos += sweep_dir_by_name(&store.repos_dir(), &used_repo_keys).await?;
    removed_hooks += sweep_dir_by_name(&store.hooks_dir(), &used_hook_env_dirs).await?;

    // Sweep tools/<bucket>
    let tools_root = store.path().join("tools");
    let used_tool_names: FxHashSet<String> =
        used_tools.iter().map(|t| t.as_str().to_string()).collect();
    removed_tools += sweep_dir_by_name(&tools_root, &used_tool_names).await?;

    // Sweep cache/<bucket>
    let cache_root = store.path().join("cache");
    let used_cache_names: FxHashSet<String> =
        used_cache.iter().map(|c| c.as_str().to_string()).collect();
    removed_cache += sweep_dir_by_name(&cache_root, &used_cache_names).await?;

    // Always clear scratch and patches; they are temporary workspaces.
    let _ = remove_dir_if_exists(&store.scratch_path()).await?;
    let _ = remove_dir_if_exists(&store.patches_dir()).await?;

    writeln!(
        printer.stdout(),
        "Removed {removed_repos} repos, {removed_hooks} hook envs, {removed_tools} tools, {removed_cache} caches"
    )?;

    Ok(ExitStatus::Success)
}

async fn build_hooks_from_config(store: &Store, config_path: &Path) -> Result<Vec<Hook>> {
    let project = Arc::new(Project::from_config_file(Cow::Borrowed(config_path), None)?);
    let config = project.config();

    let mut hooks = Vec::new();
    for repo_config in &config.repos {
        match repo_config {
            ConfigRepo::Remote(repo_config) => {
                // Only use already-cloned repos. Do not clone new ones.
                let repo_path = store.repo_path(repo_config);
                if !repo_path.is_dir() {
                    continue;
                }

                let repo = match HookRepo::remote(
                    repo_config.repo.clone(),
                    repo_config.rev.clone(),
                    repo_path,
                ) {
                    Ok(repo) => Arc::new(repo),
                    Err(err) => {
                        warn!(repo = %repo_config.repo, %err, "Failed to load repo manifest, skipping");
                        continue;
                    }
                };

                for hook_config in &repo_config.hooks {
                    let Some(hook_spec) = repo.get_hook(&hook_config.id) else {
                        continue;
                    };

                    let mut builder = HookBuilder::new(
                        Arc::clone(&project),
                        Arc::clone(&repo),
                        hook_spec.clone(),
                        hooks.len(),
                    );
                    builder.update(hook_config);
                    builder.combine(config);
                    if let Ok(hook) = builder.build().await {
                        hooks.push(hook);
                    }
                }
            }
            ConfigRepo::Local(repo_config) => {
                let repo = Arc::new(HookRepo::local(repo_config.hooks.clone()));
                for hook_config in &repo_config.hooks {
                    let hook_spec = HookSpec::from(hook_config.clone());
                    let mut builder = HookBuilder::new(
                        Arc::clone(&project),
                        Arc::clone(&repo),
                        hook_spec,
                        hooks.len(),
                    );
                    builder.combine(config);
                    if let Ok(hook) = builder.build().await {
                        hooks.push(hook);
                    }
                }
            }
            ConfigRepo::Meta(repo_config) => {
                let repo = Arc::new(HookRepo::meta(repo_config.hooks.clone()));
                for hook_config in &repo_config.hooks {
                    let hook_spec = HookSpec::from(hook_config.clone());
                    let mut builder = HookBuilder::new(
                        Arc::clone(&project),
                        Arc::clone(&repo),
                        hook_spec,
                        hooks.len(),
                    );
                    builder.combine(config);
                    if let Ok(hook) = builder.build().await {
                        hooks.push(hook);
                    }
                }
            }
            ConfigRepo::Builtin(repo_config) => {
                let repo = Arc::new(HookRepo::builtin(repo_config.hooks.clone()));
                for hook_config in &repo_config.hooks {
                    let hook_spec = HookSpec::from(hook_config.clone());
                    let mut builder = HookBuilder::new(
                        Arc::clone(&project),
                        Arc::clone(&repo),
                        hook_spec,
                        hooks.len(),
                    );
                    builder.combine(config);
                    if let Ok(hook) = builder.build().await {
                        hooks.push(hook);
                    }
                }
            }
        }
    }

    Ok(hooks)
}

fn mark_tools_and_cache_for_hook(
    hook: &Hook,
    used_tools: &mut FxHashSet<ToolBucket>,
    used_cache: &mut FxHashSet<CacheBucket>,
) {
    match hook.language {
        Language::Python | Language::Pygrep => {
            used_tools.insert(ToolBucket::Uv);
            used_tools.insert(ToolBucket::Python);
            used_cache.insert(CacheBucket::Uv);
            used_cache.insert(CacheBucket::Python);
        }
        Language::Node => {
            used_tools.insert(ToolBucket::Node);
        }
        Language::Golang => {
            used_tools.insert(ToolBucket::Go);
            used_cache.insert(CacheBucket::Go);
        }
        Language::Ruby => {
            used_tools.insert(ToolBucket::Ruby);
        }
        Language::Rust => {
            used_tools.insert(ToolBucket::Rustup);
            used_cache.insert(CacheBucket::Cargo);
        }
        _ => {}
    }
}

async fn remove_dir_if_exists(path: &Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    if path.is_dir() {
        fs_err::tokio::remove_dir_all(path).await?;
    } else {
        fs_err::tokio::remove_file(path).await?;
    }
    Ok(true)
}

async fn sweep_dir_by_name(root: &Path, keep_names: &FxHashSet<String>) -> Result<usize> {
    if !root.exists() {
        return Ok(0);
    }

    let mut removed = 0usize;
    let entries = match fs_err::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(err) => return Err(err.into()),
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                warn!(%err, root = %root.display(), "Failed to read store entry");
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if keep_names.contains(name) {
            continue;
        }

        // Best-effort cleanup.
        if let Err(err) = fs_err::tokio::remove_dir_all(&path).await {
            warn!(%err, path = %path.display(), "Failed to remove unused cache entry");
        } else {
            removed += 1;
        }
    }

    Ok(removed)
}
