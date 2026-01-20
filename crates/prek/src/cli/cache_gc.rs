use std::fmt::Write;
use std::fmt::{Display, Formatter};
use std::ops::AddAssign;
use std::path::Path;

use anyhow::Result;
use owo_colors::OwoColorize;
use rustc_hash::FxHashSet;
use tracing::{debug, trace, warn};

use crate::cli::ExitStatus;
use crate::cli::cache_size::{dir_size_bytes, human_readable_bytes};
use crate::config::{self, Error as ConfigError, Language, Repo as ConfigRepo, load_config};
use crate::hook::{HOOK_MARKER, HookEnvKey, HookSpec, InstallInfo, Repo as HookRepo};
use crate::printer::Printer;
use crate::store::{CacheBucket, REPO_MARKER, Store, ToolBucket};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum RemovalKind {
    Repos,
    HookEnvs,
    Tools,
    CacheEntries,
}

impl RemovalKind {
    fn as_str(self) -> &'static str {
        match self {
            RemovalKind::Repos => "repos",
            RemovalKind::HookEnvs => "hook envs",
            RemovalKind::Tools => "tools",
            RemovalKind::CacheEntries => "cache entries",
        }
    }
}

#[derive(Debug, Clone)]
struct Removal {
    kind: RemovalKind,
    count: usize,
    bytes: u64,
    items: Vec<RemovalItem>,
}

impl Removal {
    fn new(kind: RemovalKind) -> Self {
        Self {
            kind,
            count: 0,
            bytes: 0,
            items: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.count == 0
    }
}

impl Display for Removal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}", self.count.cyan().bold(), self.kind.as_str())
    }
}

#[derive(Debug, Clone)]
struct RemovalItem {
    label: String,
    abs_path: String,
    lines: Vec<String>,
}

impl RemovalItem {
    fn new(label: String, abs_path: String) -> Self {
        Self {
            label,
            abs_path,
            lines: Vec::new(),
        }
    }
}

#[derive(Debug, Default)]
struct RemovalSummary {
    parts: Vec<String>,
    count: usize,
    bytes: u64,
}

impl RemovalSummary {
    fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }

    fn joined(&self) -> String {
        self.parts.join(", ")
    }

    fn total_bytes(&self) -> u64 {
        self.bytes
    }
}

impl AddAssign<&Removal> for RemovalSummary {
    fn add_assign(&mut self, rhs: &Removal) {
        if rhs.count > 0 {
            self.parts.push(rhs.to_string());
        }
        self.count += rhs.count;
        self.bytes = self.bytes.saturating_add(rhs.bytes);
    }
}

pub(crate) async fn cache_gc(
    store: &Store,
    dry_run: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let _lock = store.lock_async().await?;

    let tracked_configs = store.tracked_configs()?;
    if tracked_configs.is_empty() {
        writeln!(printer.stdout(), "{}", "Nothing to clean".bold())?;
        return Ok(ExitStatus::Success);
    }

    let mut kept_configs: FxHashSet<&Path> = FxHashSet::default();
    let mut used_repo_keys: FxHashSet<String> = FxHashSet::default();
    let mut used_hook_env_dirs: FxHashSet<String> = FxHashSet::default();
    let mut used_tools: FxHashSet<ToolBucket> = FxHashSet::default();
    let mut used_cache: FxHashSet<CacheBucket> = FxHashSet::default();
    let mut used_env_keys: Vec<HookEnvKey> = Vec::new();

    // Always keep Prek's own cache.
    used_cache.insert(CacheBucket::Prek);

    let installed = store.installed_hooks().await;

    for config_path in &tracked_configs {
        let config = match load_config(config_path) {
            Ok(config) => {
                trace!(path = %config_path.display(), "Found tracked config");
                config
            }
            Err(err) => match err {
                ConfigError::NotFound(_) => {
                    debug!(path = %config_path.display(), "Tracked config does not exist, dropping");
                    continue;
                }
                err => {
                    warn!(path = %config_path.display(), %err, "Failed to parse config, skipping for GC");
                    kept_configs.insert(config_path);
                    continue;
                }
            },
        };
        kept_configs.insert(config_path);

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
        let kept_configs = kept_configs.into_iter().map(Path::to_path_buf).collect();
        store.update_tracked_configs(&kept_configs)?;
    }

    // Sweep repos/<hash>
    let removed_repos = sweep_dir_by_name(
        RemovalKind::Repos,
        &store.repos_dir(),
        &used_repo_keys,
        dry_run,
        verbose,
    )?;

    // Sweep hooks/<hash>
    let removed_hooks = sweep_dir_by_name(
        RemovalKind::HookEnvs,
        &store.hooks_dir(),
        &used_hook_env_dirs,
        dry_run,
        verbose,
    )?;

    // Sweep tools/<bucket>
    let tools_root = store.tools_dir();
    let used_tool_names: FxHashSet<String> =
        used_tools.iter().map(|t| t.as_str().to_string()).collect();
    let removed_tools = sweep_dir_by_name(
        RemovalKind::Tools,
        &tools_root,
        &used_tool_names,
        dry_run,
        verbose,
    )?;

    // Sweep cache/<bucket>
    let cache_root = store.cache_dir();
    let used_cache_names: FxHashSet<String> =
        used_cache.iter().map(|c| c.as_str().to_string()).collect();
    let removed_cache = sweep_dir_by_name(
        RemovalKind::CacheEntries,
        &cache_root,
        &used_cache_names,
        dry_run,
        verbose,
    )?;

    // Always clear scratch, as it is only temporary data.
    if !dry_run {
        let _ = fs_err::remove_dir_all(store.scratch_path());
    }
    // NOTE: Do not clear `patches/` here. It can contain user-important temporary patches.
    // A future enhancement could implement a safer cleanup strategy (e.g. GC patches older
    // than a configurable age, or only remove patches known to be orphaned).
    // let _ = fs_err::remove_dir_all(store.patches_dir())?;

    let mut removed = RemovalSummary::default();
    removed += &removed_repos;
    removed += &removed_hooks;
    removed += &removed_tools;
    removed += &removed_cache;

    let removed_total_bytes = removed.total_bytes();
    let (removed_bytes, removed_unit) = human_readable_bytes(removed_total_bytes);

    let verb = if dry_run { "Would remove" } else { "Removed" };
    if removed.is_empty() {
        writeln!(printer.stdout(), "{}", "Nothing to clean".bold())?;
    } else {
        writeln!(
            printer.stdout(),
            "{verb} {} ({}{removed_unit})",
            removed.joined(),
            format!("{removed_bytes:.1}").cyan().bold(),
        )?;

        if verbose {
            print_removed_details(printer, verb, &removed_repos)?;
            print_removed_details(printer, verb, &removed_hooks)?;
            print_removed_details(printer, verb, &removed_tools)?;
            print_removed_details(printer, verb, &removed_cache)?;
        }
    }

    Ok(ExitStatus::Success)
}

fn print_removed_details(printer: Printer, verb: &str, removal: &Removal) -> Result<()> {
    if removal.count == 0 {
        return Ok(());
    }

    writeln!(
        printer.stdout(),
        "\n{}:",
        format!("{verb} {} {}", removal.count.cyan(), removal.kind.as_str()).bold()
    )?;

    let mut items = removal.items.clone();
    items.sort_by(|a, b| a.label.cmp(&b.label));
    for item in items {
        writeln!(
            printer.stdout(),
            "- {}\n  path: {}",
            item.label,
            item.abs_path
        )?;
        for line in item.lines {
            writeln!(printer.stdout(), "  {line}")?;
        }
    }

    Ok(())
}

fn hook_env_keys_from_config(store: &Store, config: &config::Config) -> Vec<HookEnvKey> {
    let mut keys = Vec::new();

    for repo_config in &config.repos {
        match repo_config {
            ConfigRepo::Remote(repo_config) => {
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
                for hook in &repo_config.hooks {
                    let hook_spec = HookSpec::from(hook.clone());
                    match HookEnvKey::from_hook_spec(config, hook_spec, None) {
                        Ok(Some(key)) => keys.push(key),
                        Ok(None) => {}
                        Err(err) => {
                            warn!(hook = %hook.id, %err, "Failed to compute hook env key, skipping");
                        }
                    }
                }
            }
            _ => {} // Meta repos and builtin repos do not have hook envs.
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

fn sweep_dir_by_name(
    kind: RemovalKind,
    root: &Path,
    keep_names: &FxHashSet<String>,
    dry_run: bool,
    collect_names: bool,
) -> Result<Removal> {
    let mut removal = Removal::new(kind);
    let entries = match fs_err::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Removal::new(kind)),
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

        let entry_bytes = dir_size_bytes(&path);

        let item = if collect_names {
            let mut item = RemovalItem::new(name.to_string(), path.to_string_lossy().to_string());
            if let Some(label) = label_for_entry(kind, &path) {
                item.label = label;
            }
            item.lines = detail_lines_for_entry(kind, &path);
            Some(item)
        } else {
            None
        };

        if dry_run {
            removal.count += 1;
            removal.bytes = removal.bytes.saturating_add(entry_bytes);
            if collect_names {
                if let Some(item) = item {
                    removal.items.push(item);
                }
            }
            continue;
        }

        // Best-effort cleanup.
        if let Err(err) = fs_err::remove_dir_all(&path) {
            warn!(%err, path = %path.display(), "Failed to remove unused cache entry");
        } else {
            removal.count += 1;
            removal.bytes = removal.bytes.saturating_add(entry_bytes);
            if collect_names {
                if let Some(item) = item {
                    removal.items.push(item);
                }
            }
        }
    }

    Ok(removal)
}

fn label_for_entry(kind: RemovalKind, root: &Path) -> Option<String> {
    match kind {
        RemovalKind::Repos => {
            read_repo_marker(root).map(|repo| format!("{}@{}", repo.repo, repo.rev))
        }
        RemovalKind::HookEnvs => read_hook_marker(root).map(|info| {
            // Keep this short; more info goes in detail lines.
            format!("{} env", info.language.as_str())
        }),
        _ => None,
    }
}

fn detail_lines_for_entry(kind: RemovalKind, root: &Path) -> Vec<String> {
    const MAX_VALUE_CHARS: usize = 140;

    match kind {
        RemovalKind::Repos => {
            let Some(repo) = read_repo_marker(root) else {
                return Vec::new();
            };

            let mut lines = Vec::new();
            lines.push(format!("url: {}", repo.repo));
            lines.push(format!("rev: {}", repo.rev));
            lines
        }
        RemovalKind::HookEnvs => {
            let Some(info) = read_hook_marker(root) else {
                return Vec::new();
            };

            let mut lines = Vec::new();
            lines.push(format!(
                "language: {} ({})",
                info.language.as_str(),
                info.language_version
            ));

            let (repo_dep, deps) = split_repo_dependency(&info.dependencies);
            if let Some(repo_dep) = repo_dep {
                lines.push(format!("repo: {repo_dep}"));
            }
            if !deps.is_empty() {
                let deps_str = format_dependency_list(&deps, 6, MAX_VALUE_CHARS);
                lines.push(format!("deps: {deps_str}"));
            }
            lines
        }
        _ => Vec::new(),
    }
}

#[derive(Debug, serde::Deserialize)]
struct RepoMarker {
    repo: String,
    rev: String,
}

fn read_repo_marker(root: &Path) -> Option<RepoMarker> {
    // NOTE: `Store::clone_repo` serializes `RemoteRepo`, but with some fields skipped during
    // serialization (e.g. `hooks`). That means deserializing back into `RemoteRepo` can fail.
    // For GC display, we only need `repo` + `rev`.
    let content = fs_err::read_to_string(root.join(REPO_MARKER)).ok()?;
    serde_json::from_str(&content).ok()
}

fn read_hook_marker(root: &Path) -> Option<InstallInfo> {
    let content = fs_err::read_to_string(root.join(HOOK_MARKER)).ok()?;
    serde_json::from_str(&content).ok()
}

fn truncate_end(s: &str, max_chars: usize) -> String {
    if s.chars().count() <= max_chars {
        return s.to_string();
    }
    let mut out = s
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

fn split_repo_dependency(deps: &FxHashSet<String>) -> (Option<String>, Vec<String>) {
    // Best-effort: the remote repo dependency is typically `repo@rev`.
    // Prefer URL-like values to avoid accidentally treating PEP508 deps as repo identifiers.
    let mut repo_dep: Option<String> = None;
    let mut rest = Vec::new();

    for dep in deps {
        if repo_dep.is_none()
            && dep.contains('@')
            && (dep.contains("://")
                || dep.starts_with('/')
                || dep.starts_with("..")
                || dep.starts_with('.'))
        {
            repo_dep = Some(dep.clone());
        } else {
            rest.push(dep.clone());
        }
    }

    rest.sort_unstable();
    (repo_dep, rest)
}

fn format_dependency_list(deps: &[String], max_items: usize, max_chars: usize) -> String {
    if deps.is_empty() {
        return String::new();
    }

    let shown: Vec<&str> = deps.iter().take(max_items).map(String::as_str).collect();
    let extra = deps.len().saturating_sub(shown.len());
    let mut rendered = shown.join(", ");
    if extra > 0 {
        let _ = write!(&mut rendered, ", … (+{extra} more)");
    }
    truncate_end(&rendered, max_chars)
}
