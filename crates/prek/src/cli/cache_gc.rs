use std::fmt::Write;
use std::fmt::{Display, Formatter};
use std::ops::AddAssign;
use std::path::Path;
use std::time::{Duration, SystemTime};

use anyhow::Result;
use owo_colors::OwoColorize;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use strum::IntoEnumIterator;
use tracing::{debug, trace, warn};

use crate::cli::ExitStatus;
use crate::cli::cache_size::{dir_size_bytes, human_readable_bytes};
use crate::cli::run::InstallCache;
use crate::config::{self, Error as ConfigError, Repo as ConfigRepo, load_config};
use crate::hook::{HookEnvRequirement, HookSpec, InstallInfo, Repo as HookRepo, RepoIdentityRef};
use crate::printer::Printer;
use crate::store::{CacheBucket, ExclusiveStore, RepoMarker, Store, StoreWriteGuard, ToolBucket};

#[derive(Debug, Copy, Clone, Eq, PartialEq)]
enum RemovalKind {
    Repos,
    HookEnvs,
    Tools,
    CacheEntries,
    PatchFiles,
}

impl RemovalKind {
    fn display(self, count: usize) -> &'static str {
        if count > 1 {
            match self {
                RemovalKind::Repos => "repos",
                RemovalKind::HookEnvs => "hook envs",
                RemovalKind::Tools => "tools",
                RemovalKind::CacheEntries => "cache entries",
                RemovalKind::PatchFiles => "patch files",
            }
        } else {
            match self {
                RemovalKind::Repos => "repo",
                RemovalKind::HookEnvs => "hook env",
                RemovalKind::Tools => "tool",
                RemovalKind::CacheEntries => "cache entry",
                RemovalKind::PatchFiles => "patch file",
            }
        }
    }
}

const STALE_PATCH_RETENTION: Duration = Duration::from_hours(30 * 24);

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
}

impl Display for Removal {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {}",
            self.count.cyan().bold(),
            self.kind.display(self.count)
        )
    }
}

impl AddAssign for Removal {
    fn add_assign(&mut self, rhs: Self) {
        debug_assert_eq!(self.kind, rhs.kind);

        self.count += rhs.count;
        self.bytes = self.bytes.saturating_add(rhs.bytes);
        self.items.extend(rhs.items);
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

pub(crate) enum GcMode<'a> {
    DryRun(StoreWriteGuard<'a>),
    Remove(ExclusiveStore),
}

impl GcMode<'_> {
    pub(crate) fn store(&self) -> &Store {
        match self {
            Self::DryRun(store) => store,
            Self::Remove(store) => store,
        }
    }
}

pub(crate) async fn cache_gc(
    mode: GcMode<'_>,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let store = mode.store();
    let tracked_configs = store.tracked_configs()?;
    let dry_run = matches!(mode, GcMode::DryRun(_));
    // Update checks can populate the cache before any hook config is tracked.
    let update_caches = sweep_dir_by_name(
        RemovalKind::CacheEntries,
        &store.updates_dir(),
        &FxHashSet::default(),
        dry_run,
        verbose,
    )?;
    if tracked_configs.is_empty() {
        return report_removals(&[&update_caches], dry_run, verbose, printer);
    }

    let mut kept_configs: FxHashSet<&Path> = FxHashSet::default();
    let mut used_repo_keys: FxHashSet<String> = FxHashSet::default();
    let mut used_hook_env_dirs: FxHashSet<String> = FxHashSet::default();
    let mut used_tools: FxHashSet<ToolBucket> = FxHashSet::default();
    let mut used_tool_versions: FxHashMap<ToolBucket, FxHashSet<String>> = FxHashMap::default();
    let mut used_cache: FxHashSet<CacheBucket> = FxHashSet::default();
    let mut used_env_requirements: Vec<HookEnvRequirement> = Vec::new();

    // Always keep Prek's own cache.
    used_cache.insert(CacheBucket::Prek);

    let install_cache = InstallCache::new();

    for config_path in &tracked_configs {
        let config = match load_config(config_path) {
            Ok(config) => {
                trace!(path = %config_path.display(), "Found tracked config");
                config
            }
            Err(err) => match err {
                ConfigError::Io(e) if e.kind() == std::io::ErrorKind::NotFound => {
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

        used_env_requirements.extend(hook_env_requirements_from_config(store, &config));

        // Mark repos referenced by this config (if present in store).
        // We do this via config parsing (no clone), so GC won't keep repos for missing configs.
        for repo in &config.repos {
            if let ConfigRepo::Remote(remote) = repo {
                used_repo_keys.insert(Store::repo_key(remote.source(), &remote.rev));
            }
        }
    }

    // Mark tools/caches from hook languages.
    for requirement in &used_env_requirements {
        used_tools.extend(requirement.language.tool_buckets());
        used_cache.extend(requirement.language.cache_buckets());
    }

    // Mark hook environments by matching already-installed env metadata.
    // While doing this, try to derive the specific tool *version* directories in use from
    // `InstallInfo.toolchain` (which is persisted in `.prek-hook.json`).
    for installed in install_cache.installed_hooks(store).await {
        let info = installed.info_ref();
        if used_env_requirements
            .iter()
            .any(|requirement| requirement.as_ref().is_satisfied_by(info))
        {
            if let Some(dir) = info
                .env_path
                .file_name()
                .and_then(|s| s.to_str())
                .map(str::to_string)
            {
                used_hook_env_dirs.insert(dir);
            }

            mark_tool_versions_from_install_info(store, info, &mut used_tool_versions);
        }
    }

    // Update tracking file to drop configs that no longer exist.
    if let GcMode::Remove(exclusive) = &mode
        && kept_configs.len() != tracked_configs.len()
    {
        let kept_configs = kept_configs.into_iter().map(Path::to_path_buf).collect();
        exclusive.update_tracked_configs(&kept_configs)?;
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
    let used_tool_names: FxHashSet<String> = used_tools.iter().map(ToString::to_string).collect();
    let removed_tool_buckets = sweep_dir_by_name(
        RemovalKind::Tools,
        &tools_root,
        &used_tool_names,
        dry_run,
        verbose,
    )?;

    // Sweep tools/<bucket>/<version>
    let removed_tool_versions = sweep_tool_versions(store, &used_tool_versions, dry_run, verbose)?;

    let mut removed_tools = removed_tool_buckets;
    removed_tools += removed_tool_versions;

    // Sweep cache/<bucket>
    let cache_root = store.cache_dir();
    let used_cache_names: FxHashSet<String> = used_cache.iter().map(ToString::to_string).collect();
    let mut removed_cache = sweep_dir_by_name(
        RemovalKind::CacheEntries,
        &cache_root,
        &used_cache_names,
        dry_run,
        verbose,
    )?;

    removed_cache += update_caches;

    if !dry_run {
        let _ = fs_err::remove_dir_all(store.scratch_path());
    }
    // Keep recent recovery patches, but clear out stale ones that are unlikely to be useful.
    let removed_patches = sweep_stale_patch_files(&store.patches_dir(), dry_run, verbose)?;

    report_removals(
        &[
            &removed_repos,
            &removed_hooks,
            &removed_tools,
            &removed_cache,
            &removed_patches,
        ],
        dry_run,
        verbose,
        printer,
    )
}

fn report_removals(
    removals: &[&Removal],
    dry_run: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let mut removed = RemovalSummary::default();
    for &removal in removals {
        removed += removal;
    }
    let removed_total_bytes = removed.total_bytes();
    let (removed_bytes, removed_unit) = human_readable_bytes(removed_total_bytes);

    let verb = if dry_run { "Would remove" } else { "Removed" };
    if removed.is_empty() {
        writeln!(printer.stdout(), "{}", "Nothing to clean".bold())?;
    } else {
        writeln!(
            printer.stdout(),
            "{verb} {} ({})",
            removed.joined(),
            format!("{removed_bytes:.1}{removed_unit}").cyan().bold(),
        )?;

        if verbose {
            for removal in removals {
                print_removed_details(printer, verb, removal)?;
            }
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
        format!("{verb} {removal}").bold()
    )?;

    let mut items = removal.items.clone();
    items.sort_unstable_by(|a, b| a.label.cmp(&b.label));
    for item in items {
        writeln!(printer.stdout(), "{} {}", "-".dimmed(), item.label.bold())?;
        writeln!(
            printer.stdout(),
            "  {}: {}",
            "path".bold().dimmed(),
            item.abs_path
        )?;

        for line in item.lines {
            writeln!(printer.stdout(), "  {line}")?;
        }
    }

    Ok(())
}

fn hook_env_requirements_from_config(
    store: &Store,
    config: &config::Config,
) -> Vec<HookEnvRequirement> {
    let mut requirements = Vec::new();

    for repo_config in &config.repos {
        match repo_config {
            ConfigRepo::Remote(repo_config) => {
                let repo_path = store.repo_path(repo_config.source(), &repo_config.rev);
                if !repo_path.is_dir() {
                    continue;
                }

                let repo = match HookRepo::remote(
                    repo_config.source().to_string(),
                    repo_config.rev.clone(),
                    repo_path,
                ) {
                    Ok(repo) => repo,
                    Err(err) => {
                        warn!(repo = %repo_config.repo(), %err, "Failed to load repo manifest, skipping");
                        continue;
                    }
                };

                let remote_repo = RepoIdentityRef::new(repo_config.source(), &repo_config.rev);

                for hook_config in &repo_config.hooks {
                    let Some(manifest_hook) = repo.manifest_hook(&hook_config.id) else {
                        continue;
                    };

                    let hook_spec = HookSpec::from_remote(manifest_hook.clone(), hook_config);

                    match HookEnvRequirement::from_hook_spec(config, hook_spec, Some(remote_repo)) {
                        Ok(Some(requirement)) => requirements.push(requirement),
                        Ok(None) => {}
                        Err(err) => {
                            warn!(hook = %hook_config.id, repo = %remote_repo, %err, "Failed to compute hook environment requirement, skipping");
                        }
                    }
                }
            }
            ConfigRepo::Local(repo_config) => {
                for hook in &repo_config.hooks {
                    let hook_spec = HookSpec::from(hook.clone());
                    match HookEnvRequirement::from_hook_spec(config, hook_spec, None) {
                        Ok(Some(requirement)) => requirements.push(requirement),
                        Ok(None) => {}
                        Err(err) => {
                            warn!(hook = %hook.id, %err, "Failed to compute hook environment requirement, skipping");
                        }
                    }
                }
            }
            _ => {} // Meta repos and builtin repos do not have hook envs.
        }
    }

    requirements
}

fn mark_tool_versions_from_install_info(
    store: &Store,
    info: &InstallInfo,
    used_tool_versions: &mut FxHashMap<ToolBucket, FxHashSet<String>>,
) {
    // NOTE: `InstallInfo.toolchain` is typically the executable path (e.g.
    // tools/go/1.24.0/bin/go). We keep the first path component under the tool bucket.
    // If we can't recognize it, we do nothing (and GC will keep all versions).
    for bucket in info.language.tool_buckets() {
        let bucket_root = store.tools_path(*bucket);
        if let Some(version) = tool_version_dir_name(&bucket_root, &info.toolchain) {
            used_tool_versions
                .entry(*bucket)
                .or_default()
                .insert(version);
        }
    }
}

fn tool_version_dir_name(bucket_root: &Path, toolchain: &Path) -> Option<String> {
    let rel = toolchain.strip_prefix(bucket_root).ok()?;
    let version = rel.components().next()?.as_os_str().to_str()?;
    if version.is_empty() {
        return None;
    }
    Some(version.to_string())
}

fn sweep_tool_versions(
    store: &Store,
    used_tool_versions: &FxHashMap<ToolBucket, FxHashSet<String>>,
    dry_run: bool,
    verbose: bool,
) -> Result<Removal> {
    let mut total = Removal::new(RemovalKind::Tools);

    for bucket in ToolBucket::iter() {
        let bucket_root = store.tools_path(bucket);
        let keep_versions = used_tool_versions.get(&bucket);
        let removed =
            sweep_tool_bucket_versions(bucket, &bucket_root, keep_versions, dry_run, verbose)?;
        total += removed;
    }

    Ok(total)
}

fn sweep_tool_bucket_versions(
    bucket: ToolBucket,
    bucket_root: &Path,
    keep_versions: Option<&FxHashSet<String>>,
    dry_run: bool,
    collect_names: bool,
) -> Result<Removal> {
    let mut removal = Removal::new(RemovalKind::Tools);

    let entries = match fs_err::read_dir(bucket_root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Removal::new(RemovalKind::Tools));
        }
        Err(err) => return Err(err.into()),
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(err) => {
                warn!(%err, root = %bucket_root.display(), "Failed to read tool bucket entry");
                continue;
            }
        };
        let path = entry.path();
        // Don't remove files (uv, and rustup are files inside tools/).
        if !path.is_dir() {
            continue;
        }

        let Some(version_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        // Skip hidden/system dirs.
        if version_name.starts_with('.') {
            continue;
        }
        if keep_versions.is_some_and(|keep| keep.contains(version_name)) {
            continue;
        }

        let entry_bytes = dir_size_bytes(&path);

        let item = if collect_names {
            Some(RemovalItem::new(
                format!("{bucket}/{version_name}"),
                path.to_string_lossy().into_owned(),
            ))
        } else {
            None
        };

        if dry_run {
            removal.count += 1;
            removal.bytes = removal.bytes.saturating_add(entry_bytes);
            if let Some(item) = item {
                removal.items.push(item);
            }
            continue;
        }

        if let Err(err) = fs_err::remove_dir_all(&path) {
            warn!(%err, path = %path.display(), "Failed to remove unused tool version");
        } else {
            removal.count += 1;
            removal.bytes = removal.bytes.saturating_add(entry_bytes);
            if let Some(item) = item {
                removal.items.push(item);
            }
        }
    }

    Ok(removal)
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
        // Skip hidden/system dirs.
        if name.starts_with('.') {
            continue;
        }
        if keep_names.contains(name) {
            continue;
        }

        let entry_bytes = dir_size_bytes(&path);

        let item = if collect_names {
            let mut item = RemovalItem::new(name.to_string(), path.to_string_lossy().into_owned());
            match kind {
                RemovalKind::Repos => {
                    if let Some(repo) = RepoMarker::read(&path) {
                        item.label = format!("{}@{}", repo.repo, repo.rev);
                    }
                }
                RemovalKind::HookEnvs => {
                    if let Some(info) = InstallInfo::read(&path) {
                        item.label = format!("{} env", info.language.as_ref());
                        item.lines = hook_detail_lines(&info);
                    }
                }
                _ => {}
            }
            Some(item)
        } else {
            None
        };

        if dry_run {
            removal.count += 1;
            removal.bytes = removal.bytes.saturating_add(entry_bytes);
            if collect_names && let Some(item) = item {
                removal.items.push(item);
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

fn sweep_stale_patch_files(root: &Path, dry_run: bool, collect_names: bool) -> Result<Removal> {
    let mut removal = Removal::new(RemovalKind::PatchFiles);
    let entries = match fs_err::read_dir(root) {
        Ok(entries) => entries,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Removal::new(RemovalKind::PatchFiles));
        }
        Err(err) => return Err(err.into()),
    };

    let cutoff = SystemTime::now()
        .checked_sub(STALE_PATCH_RETENTION)
        .unwrap_or(SystemTime::UNIX_EPOCH);

    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };

        if !entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
            continue;
        }

        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if name.starts_with('.') || path.extension().and_then(|ext| ext.to_str()) != Some("patch") {
            continue;
        }

        let metadata = match entry.metadata() {
            Ok(metadata) => metadata,
            Err(err) => {
                warn!(%err, path = %path.display(), "Failed to read patch metadata");
                continue;
            }
        };
        let modified = match metadata.modified() {
            Ok(modified) => modified,
            Err(err) => {
                warn!(%err, path = %path.display(), "Failed to read patch modified time");
                continue;
            }
        };
        if modified > cutoff {
            continue;
        }

        let entry_bytes = metadata.len();
        let item = collect_names
            .then(|| RemovalItem::new(name.to_string(), path.to_string_lossy().into_owned()));

        if dry_run {
            removal.count += 1;
            removal.bytes = removal.bytes.saturating_add(entry_bytes);
            if let Some(item) = item {
                removal.items.push(item);
            }
            continue;
        }

        if let Err(err) = fs_err::remove_file(&path) {
            warn!(%err, path = %path.display(), "Failed to remove old patch file");
        } else {
            removal.count += 1;
            removal.bytes = removal.bytes.saturating_add(entry_bytes);
            if let Some(item) = item {
                removal.items.push(item);
            }
        }
    }

    Ok(removal)
}

fn hook_detail_lines(info: &InstallInfo) -> Vec<String> {
    const MAX_VALUE_CHARS: usize = 140;

    let mut lines = Vec::new();
    lines.push(format!(
        "{}: {} ({})",
        "language".dimmed().bold(),
        info.language.as_ref(),
        info.language_version
    ));

    if let Some(repo) = info.repo() {
        lines.push(format!(
            "{}: {}",
            "repo".dimmed().bold(),
            truncate_end(&repo.to_string(), MAX_VALUE_CHARS)
        ));
    }
    if !info.dependencies.is_empty() {
        let deps_str = format_dependency_list(&info.dependencies, 6, MAX_VALUE_CHARS);
        lines.push(format!("{}: {deps_str}", "deps".dimmed().bold()));
    }
    lines
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_end_returns_input_when_short_enough() {
        assert_eq!(truncate_end("abc", 3), "abc");
        assert_eq!(truncate_end("abc", 10), "abc");
    }

    #[test]
    fn truncate_end_truncates_and_appends_ellipsis() {
        assert_eq!(truncate_end("abcd", 3), "ab…");
        assert_eq!(truncate_end("abcdef", 5), "abcd…");
    }

    #[test]
    fn truncate_end_counts_chars_not_bytes() {
        // 3 unicode scalar values.
        assert_eq!(truncate_end("ééé", 3), "ééé");
        assert_eq!(truncate_end("ééé", 2), "é…");
    }

    #[test]
    fn format_dependency_list_includes_more_suffix() {
        let deps = vec!["a".to_string(), "b".to_string(), "c".to_string()];
        assert_eq!(format_dependency_list(&deps, 2, 200), "a, b, … (+1 more)");
    }

    #[test]
    fn format_dependency_list_truncates_rendered_string() {
        let deps = vec!["abcdef".to_string()];
        assert_eq!(format_dependency_list(&deps, 6, 5), "abcd…");
    }
}
