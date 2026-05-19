use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use mea::once::OnceCell;
use mea::semaphore::Semaphore;
use rustc_hash::FxHashMap;
use tracing::{debug, warn};

use crate::cli::reporter::HookInstallReporter;
use crate::config::Language;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::run::CONCURRENCY;
use crate::store::Store;

pub(crate) async fn install_hooks(
    hooks: Vec<Arc<Hook>>,
    store: &Store,
    reporter: &HookInstallReporter,
    cache: &mut InstallCache,
) -> Result<Vec<InstalledHook>> {
    let num_hooks = hooks.len();
    let result = resolve_and_install_hooks(hooks, store, reporter, cache).await?;
    cache.add_installed(&result);

    debug_assert_eq!(
        num_hooks,
        result.len(),
        "Number of hooks installed should match the number of hooks provided"
    );

    Ok(result)
}

async fn resolve_and_install_hooks(
    hooks: Vec<Arc<Hook>>,
    store: &Store,
    reporter: &HookInstallReporter,
    cache: &mut InstallCache,
) -> Result<Vec<InstalledHook>> {
    let mut installed_hooks = Vec::with_capacity(hooks.len());
    let mut hooks_to_install = Vec::new();

    for hook in hooks {
        if !hook.needs_install_env() {
            installed_hooks.push(InstalledHook::NoNeedInstall(hook));
            continue;
        }

        if let Some(installed_hook) = cache.installed_hook(store, hook.clone()).await {
            installed_hooks.push(installed_hook);
        } else {
            hooks_to_install.push(hook);
        }
    }

    installed_hooks.extend(install_missing_hooks(hooks_to_install, store, reporter).await?);

    Ok(installed_hooks)
}

async fn install_missing_hooks(
    hooks: Vec<Arc<Hook>>,
    store: &Store,
    reporter: &HookInstallReporter,
) -> Result<Vec<InstalledHook>> {
    let semaphore = Rc::new(Semaphore::new(*CONCURRENCY));
    let mut installed_hooks = Vec::with_capacity(hooks.len());
    let mut futures = FuturesUnordered::new();

    for partition in partition_hooks(hooks) {
        let semaphore = Rc::clone(&semaphore);
        let reporter = reporter.clone();
        futures
            .push(async move { install_partition(partition, store, &reporter, semaphore).await });
    }

    while let Some(partition_hooks) = futures.next().await {
        installed_hooks.extend(partition_hooks?);
    }

    Ok(installed_hooks)
}

async fn install_partition(
    hooks: Vec<Arc<Hook>>,
    store: &Store,
    reporter: &HookInstallReporter,
    semaphore: Rc<Semaphore>,
) -> Result<Vec<InstalledHook>> {
    let mut installed_hooks = Vec::with_capacity(hooks.len());

    for hook in hooks {
        debug_assert!(hook.needs_install_env());
        let installed_hook = if let Some(info) = installed_info_for_hook(&installed_hooks, &hook) {
            debug!(
                "Found installed environment for hook `{hook}` at `{}`",
                info.env_path.display()
            );
            InstalledHook::Installed { hook, info }
        } else {
            install_new(hook, store, reporter, &semaphore).await?
        };
        installed_hooks.push(installed_hook);
    }

    Ok(installed_hooks)
}

async fn install_new(
    hook: Arc<Hook>,
    store: &Store,
    reporter: &HookInstallReporter,
    semaphore: &Semaphore,
) -> Result<InstalledHook> {
    let _permit = semaphore.acquire(1).await;

    let installed_hook = hook
        .language
        .install(hook.clone(), store, reporter)
        .await
        .with_context(|| format!("Failed to install hook `{hook}`"))?;

    installed_hook
        .mark_as_installed(store)
        .await
        .with_context(|| format!("Failed to mark hook `{hook}` as installed"))?;

    match &installed_hook {
        InstalledHook::Installed { info, .. } => {
            debug!("Installed hook `{hook}` in `{}`", info.env_path.display());
        }
        InstalledHook::NoNeedInstall { .. } => {
            debug!("Hook `{hook}` does not need installation");
        }
    }

    Ok(installed_hook)
}

fn installed_info_for_hook(
    installed_hooks: &[InstalledHook],
    hook: &Hook,
) -> Option<Arc<InstallInfo>> {
    for installed_hook in installed_hooks {
        if let InstalledHook::Installed { info, .. } = installed_hook
            && info.matches(hook)
        {
            return Some(info.clone());
        }
    }

    None
}

/// Group hooks so each partition can install independently.
///
/// Different languages can install concurrently. Hooks with the same language and dependency set
/// stay in one partition so later hooks can reuse an environment installed by an earlier hook.
fn partition_hooks(hooks: Vec<Arc<Hook>>) -> Vec<Vec<Arc<Hook>>> {
    let mut hooks_by_language = FxHashMap::default();
    for hook in hooks {
        hooks_by_language
            .entry(install_language(&hook))
            .or_insert_with(Vec::new)
            .push(hook);
    }

    let mut partitions = Vec::new();
    for (_, hooks) in hooks_by_language {
        partitions.extend(partition_hooks_by_dependencies(hooks));
    }

    partitions
}

fn install_language(hook: &Hook) -> Language {
    if hook.language == Language::Pygrep {
        // Treat `pygrep` hooks as `python` hooks for installation purposes.
        // They share the same installation logic.
        Language::Python
    } else {
        hook.language
    }
}

fn partition_hooks_by_dependencies(hooks: Vec<Arc<Hook>>) -> Vec<Vec<Arc<Hook>>> {
    let mut groups: Vec<Vec<Arc<Hook>>> = Vec::new();

    for hook in hooks {
        let group_index = groups
            .iter()
            .position(|group| group[0].env_key_dependencies() == hook.env_key_dependencies());

        if let Some(index) = group_index {
            groups[index].push(hook);
        } else {
            groups.push(vec![hook]);
        }
    }

    groups
}

#[derive(Debug, Clone)]
pub(crate) struct CachedInstallInfo {
    info: Arc<InstallInfo>,
    health: OnceCell<bool>,
}

impl CachedInstallInfo {
    fn new(info: Arc<InstallInfo>) -> Self {
        Self {
            info,
            health: OnceCell::new(),
        }
    }

    fn healthy(info: Arc<InstallInfo>) -> Self {
        Self {
            info,
            health: OnceCell::from_value(true),
        }
    }

    fn matches(&self, hook: &Hook) -> bool {
        self.info.matches(hook)
    }

    fn info(&self) -> Arc<InstallInfo> {
        self.info.clone()
    }

    pub(crate) fn info_ref(&self) -> &InstallInfo {
        &self.info
    }

    async fn ensure_healthy(&self) -> bool {
        let info = self.info.clone();
        *self
            .health
            .get_or_init(async move || match info.check_health().await {
                Ok(()) => true,
                Err(err) => {
                    warn!(
                        %err,
                        path = %info.env_path.display(),
                        "Skipping unhealthy installed hook"
                    );
                    false
                }
            })
            .await
    }
}

pub(crate) struct InstallCache {
    /// Result of the expensive hooks-dir scan. Loaded at most once per command.
    store_hooks: OnceCell<Vec<CachedInstallInfo>>,
    /// Environments installed after the scan and therefore absent from `store_hooks`.
    created_hooks: Vec<CachedInstallInfo>,
}

impl InstallCache {
    pub(crate) fn new() -> Self {
        Self {
            store_hooks: OnceCell::new(),
            created_hooks: Vec::new(),
        }
    }

    pub(crate) async fn installed_hooks<'a>(
        &'a self,
        store: &Store,
    ) -> impl Iterator<Item = &'a CachedInstallInfo> + 'a {
        let store_hooks = self.store_hooks(store).await;
        self.created_hooks.iter().chain(store_hooks.iter())
    }

    pub(crate) async fn installed_hook(
        &self,
        store: &Store,
        hook: Arc<Hook>,
    ) -> Option<InstalledHook> {
        for env in self.installed_hooks(store).await {
            if env.matches(&hook) && env.ensure_healthy().await {
                return Some(InstalledHook::Installed {
                    hook,
                    info: env.info(),
                });
            }
        }

        None
    }

    async fn store_hooks(&self, store: &Store) -> &[CachedInstallInfo] {
        self.store_hooks
            .get_or_init(async || Self::load_store_installed_hooks(store).await)
            .await
    }

    async fn load_store_installed_hooks(store: &Store) -> Vec<CachedInstallInfo> {
        match fs_err::read_dir(store.hooks_dir()) {
            Ok(dirs) => {
                let mut tasks = futures::stream::iter(dirs)
                    .map(async |entry| {
                        let path = match entry {
                            Ok(entry) => entry.path(),
                            Err(err) => {
                                warn!(%err, "Failed to read hook dir");
                                return None;
                            }
                        };
                        let info = match InstallInfo::from_env_path(&path).await {
                            Ok(info) => info,
                            Err(err) => {
                                warn!(%err, path = %path.display(), "Skipping invalid installed hook");
                                return None;
                            }
                        };
                        Some(CachedInstallInfo::new(Arc::new(info)))
                    })
                    .buffer_unordered(*CONCURRENCY);

                let mut hooks = Vec::new();
                while let Some(hook) = tasks.next().await {
                    if let Some(hook) = hook {
                        hooks.push(hook);
                    }
                }
                hooks
            }
            Err(_) => Vec::new(),
        }
    }

    pub(crate) fn add_installed(&mut self, installed_hooks: &[InstalledHook]) {
        for hook in installed_hooks {
            let InstalledHook::Installed { info, .. } = hook else {
                continue;
            };
            if self.has_info(info) {
                continue;
            }
            self.created_hooks
                .push(CachedInstallInfo::healthy(info.clone()));
        }
    }

    fn has_info(&self, info: &InstallInfo) -> bool {
        self.created_hooks
            .iter()
            .any(|existing| existing.info.env_path == info.env_path)
            || self.store_hooks.get().is_some_and(|store_hooks| {
                store_hooks
                    .iter()
                    .any(|existing| existing.info.env_path == info.env_path)
            })
    }
}
