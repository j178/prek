use std::fmt::Write;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;
use rustc_hash::FxHashSet;
use tracing::{debug, warn};

use crate::cli::ExitStatus;
use crate::config::{self, Error as ConfigError, Language, Repo as ConfigRepo, load_config};
use crate::hook::{HookEnvKey, HookSpec, Repo as HookRepo};
use crate::printer::Printer;
use crate::store::{CacheBucket, Store, ToolBucket};

pub(crate) async fn cache_gc(
    store: &Store,
    dry_run: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
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
    let mut used_env_keys: Vec<HookEnvKey> = Vec::new();

    let installed = store.installed_hooks().await;

    for config_path in &tracked_configs {
        let config = match load_config(config_path) {
            Ok(config) => config,
            Err(err) => match err {
                ConfigError::NotFound(_) => {
                    debug!(path = %config_path.display(), "Tracked config does not exist, dropping");
                    continue;
                }
                err => {
                    warn!(path = %config_path.display(), %err, "Failed to parse config, skipping for GC");
                    kept_configs.insert(config_path.clone());
                    continue;
                }
            },
        };
        kept_configs.insert(config_path.clone());

        used_env_keys.extend(hook_env_keys_from_config(store, &config));

        // Mark repos referenced by this config (if present in store).
        // We do this via config parsing (no clone), so GC won't keep repos for missing configs.
        for repo in &config.repos {
            if let ConfigRepo::Remote(remote) = repo {
                let key = Store::repo_key(remote);
                used_repo_keys.insert(key);
            }
        }
    }

    // Mark tools/caches from hook languages.
    for key in &used_env_keys {
        mark_tools_and_cache_for_language(key.language, &mut used_tools, &mut used_cache);
    }

    // Mark hook environments by matching already-installed env metadata.
    for info in &installed {
        if used_env_keys.iter().any(|k| k.matches_install_info(info)) {
            if let Some(dir) = info
                .env_path
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            {
                used_hook_env_dirs.insert(dir);
            }
        }
    }

    // Update tracking file to drop configs that no longer exist.
    if kept_configs.len() != tracked_configs.len() {
        store.update_tracked_configs(&kept_configs)?;
    }

    let (removed_repos, removed_repo_names) =
        sweep_dir_by_name(&store.repos_dir(), &used_repo_keys, dry_run, verbose).await?;
    let (removed_hooks, removed_hook_names) =
        sweep_dir_by_name(&store.hooks_dir(), &used_hook_env_dirs, dry_run, verbose).await?;

    // Sweep tools/<bucket>
    let tools_root = store.tools_dir();
    let used_tool_names: FxHashSet<String> =
        used_tools.iter().map(|t| t.as_str().to_string()).collect();
    let (removed_tools, removed_tool_names) =
        sweep_dir_by_name(&tools_root, &used_tool_names, dry_run, verbose).await?;

    // Sweep cache/<bucket>
    let cache_root = store.cache_dir();
    let used_cache_names: FxHashSet<String> =
        used_cache.iter().map(|c| c.as_str().to_string()).collect();
    let (removed_cache, removed_cache_names) =
        sweep_dir_by_name(&cache_root, &used_cache_names, dry_run, verbose).await?;

    // Always clear scratch and patches; they are temporary workspaces.
    if !dry_run {
        let _ = remove_dir_if_exists(&store.scratch_path()).await?;
    }
    // NOTE: Do not clear `patches/` here. It can contain user-important temporary patches.
    // A future enhancement could implement a safer cleanup strategy (e.g. GC patches older
    // than a configurable age, or only remove patches known to be orphaned).
    // let _ = remove_dir_if_exists(&store.patches_dir()).await?;

    let mut removed = Vec::new();
    if removed_repos > 0 {
        removed.push(format!("{} repos", removed_repos.cyan()));
    }
    if removed_hooks > 0 {
        removed.push(format!("{} hook envs", removed_hooks.cyan()));
    }
    if removed_tools > 0 {
        removed.push(format!("{} tools", removed_tools.cyan()));
    }
    if removed_cache > 0 {
        removed.push(format!("{} caches", removed_cache.cyan()));
    }

    if removed.is_empty() {
        writeln!(printer.stdout(), "Nothing to clean")?;
    } else {
        let verb = if dry_run { "Would remove" } else { "Removed" };
        writeln!(printer.stdout(), "{verb} {}", removed.join(", "))?;

        if verbose {
            print_removed_details(printer, dry_run, "repos", removed_repo_names)?;
            print_removed_details(printer, dry_run, "hooks", removed_hook_names)?;
            print_removed_details(printer, dry_run, "tools", removed_tool_names)?;
            print_removed_details(printer, dry_run, "cache", removed_cache_names)?;
        }
    }

    Ok(ExitStatus::Success)
}

fn print_removed_details(
    printer: Printer,
    dry_run: bool,
    title: &'static str,
    mut names: Vec<String>,
) -> Result<()> {
    if names.is_empty() {
        return Ok(());
    }

    names.sort();

    writeln!(
        printer.stdout(),
        "\n{} {title}:",
        if dry_run { "Would remove" } else { "Removed" }
    )?;
    for name in names {
        writeln!(printer.stdout(), "- {name}")?;
    }

    Ok(())
}

fn hook_env_keys_from_config(store: &Store, config: &config::Config) -> Vec<HookEnvKey> {
    let mut keys = Vec::new();

    fn extend_keys_from_hooks<'a, T>(
        keys: &mut Vec<HookEnvKey>,
        config: &config::Config,
        hooks: impl IntoIterator<Item = &'a T>,
        remote_dep: Option<&str>,
        mut to_spec: impl FnMut(&T) -> HookSpec,
        id_of: impl Fn(&T) -> &str,
    ) where
        T: 'a,
    {
        for hook in hooks {
            let hook_spec = to_spec(hook);
            match HookEnvKey::from_hook_spec(config, hook_spec, remote_dep) {
                Ok(Some(key)) => keys.push(key),
                Ok(None) => {}
                Err(err) => {
                    warn!(hook = %id_of(hook), %err, "Failed to compute hook env key, skipping");
                }
            }
        }
    }

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
                    Ok(repo) => repo,
                    Err(err) => {
                        warn!(repo = %repo_config.repo, %err, "Failed to load repo manifest, skipping");
                        continue;
                    }
                };

                let remote_dep = repo_config.to_string();

                for hook_config in &repo_config.hooks {
                    let Some(manifest_hook) = repo.get_hook(&hook_config.id) else {
                        continue;
                    };

                    let mut hook_spec = manifest_hook.clone();
                    hook_spec.apply_remote_hook_overrides(hook_config);

                    match HookEnvKey::from_hook_spec(config, hook_spec, Some(&remote_dep)) {
                        Ok(Some(key)) => keys.push(key),
                        Ok(None) => {}
                        Err(err) => {
                            warn!(hook = %hook_config.id, repo = %remote_dep, %err, "Failed to compute hook env key, skipping");
                        }
                    }
                }
            }
            ConfigRepo::Local(repo_config) => {
                extend_keys_from_hooks(
                    &mut keys,
                    config,
                    &repo_config.hooks,
                    None,
                    |h| HookSpec::from(h.clone()),
                    |h| h.id.as_str(),
                );
            }
            ConfigRepo::Meta(repo_config) => {
                extend_keys_from_hooks(
                    &mut keys,
                    config,
                    &repo_config.hooks,
                    None,
                    |h| HookSpec::from(h.clone()),
                    |h| h.id.as_str(),
                );
            }
            ConfigRepo::Builtin(repo_config) => {
                extend_keys_from_hooks(
                    &mut keys,
                    config,
                    &repo_config.hooks,
                    None,
                    |h| HookSpec::from(h.clone()),
                    |h| h.id.as_str(),
                );
            }
        }
    }

    keys
}

// TODO: read `toolchain` from `.prek-hook.json`, and use that to determine tools/cache to keep.
fn mark_tools_and_cache_for_language(
    language: Language,
    used_tools: &mut FxHashSet<ToolBucket>,
    used_cache: &mut FxHashSet<CacheBucket>,
) {
    match language {
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

async fn sweep_dir_by_name(
    root: &Path,
    keep_names: &FxHashSet<String>,
    dry_run: bool,
    collect_names: bool,
) -> Result<(usize, Vec<String>)> {
    if !root.exists() {
        return Ok((0, Vec::new()));
    }

    let mut removed = 0usize;
    let mut removed_names = Vec::new();
    let entries = match fs_err::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok((0, Vec::new())),
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

        if dry_run {
            removed += 1;
            if collect_names {
                removed_names.push(name.to_string());
            }
            continue;
        }

        // Best-effort cleanup.
        if let Err(err) = fs_err::tokio::remove_dir_all(&path).await {
            warn!(%err, path = %path.display(), "Failed to remove unused cache entry");
        } else {
            removed += 1;
            if collect_names {
                removed_names.push(name.to_string());
            }
        }
    }

    Ok((removed, removed_names))
}
