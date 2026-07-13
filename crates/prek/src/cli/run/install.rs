use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures_util::stream::{FuturesUnordered, StreamExt};
use mea::once::OnceCell;
use mea::semaphore::Semaphore;
use tracing::{debug, warn};

use crate::cli::reporter::HookInstallReporter;
use crate::config::Language;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::run::INTERNAL_CONCURRENCY;
use crate::store::Store;

/// Resolve already-installed hook environments and install the missing ones.
///
/// The cache is only used for environments already present in the store. Environments created
/// by this call are returned directly and reused within each install partition.
pub(crate) async fn install_hooks(
    hooks: Vec<Arc<Hook>>,
    store: &Store,
    reporter: &HookInstallReporter,
    cache: &mut InstallCache,
) -> Result<Vec<InstalledHook>> {
    let num_hooks = hooks.len();
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

    let semaphore = Rc::new(Semaphore::new(*INTERNAL_CONCURRENCY));
    let mut futures = FuturesUnordered::new();

    for partition in partition_hooks(hooks_to_install) {
        let semaphore = Rc::clone(&semaphore);
        futures.push(async move { install_partition(partition, store, reporter, semaphore).await });
    }

    while let Some(partition_hooks) = futures.next().await {
        installed_hooks.extend(partition_hooks?);
    }

    debug_assert_eq!(
        num_hooks,
        installed_hooks.len(),
        "Number of hooks installed should match the number of hooks provided"
    );

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

        let reusable_info = hook.environment_requirement().and_then(|requirement| {
            installed_hooks.iter().find_map(|installed| {
                let InstalledHook::Installed { info, .. } = installed else {
                    return None;
                };
                requirement.is_satisfied_by(info).then_some(info)
            })
        });

        let installed_hook = if let Some(info) = reusable_info {
            debug!(
                "Found installed environment for hook `{hook}` at `{}`",
                info.env_path.display()
            );
            InstalledHook::Installed {
                hook,
                info: Arc::clone(info),
            }
        } else {
            let _permit = semaphore.acquire(1).await;

            let installed_hook = hook
                .language
                .install(store, hook.clone(), reporter)
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

            installed_hook
        };
        installed_hooks.push(installed_hook);
    }

    Ok(installed_hooks)
}

/// Group hooks so each partition can install independently.
///
/// Hooks with the same install language, repository, and dependency sequence stay in one
/// partition so later hooks can reuse an environment installed by an earlier hook. Version
/// requirements are checked by the full environment requirement and intentionally do not split
/// partitions.
fn partition_hooks(hooks: Vec<Arc<Hook>>) -> Vec<Vec<Arc<Hook>>> {
    let mut partitions: Vec<Vec<Arc<Hook>>> = Vec::new();
    for hook in hooks {
        if let Some(partition) = partitions
            .iter_mut()
            .find(|partition| same_install_partition(&partition[0], &hook))
        {
            partition.push(hook);
        } else {
            partitions.push(vec![hook]);
        }
    }

    partitions
}

fn same_install_partition(left: &Hook, right: &Hook) -> bool {
    partition_language(left.language) == partition_language(right.language)
        && left.repo().identity() == right.repo().identity()
        && left.additional_dependencies == right.additional_dependencies
}

fn partition_language(language: Language) -> Language {
    // Both `pygrep` and Python may provision Python, so schedule them as one language.
    if language == Language::Pygrep {
        Language::Python
    } else {
        language
    }
}

/// Cached metadata for one environment found in the store hooks directory.
///
/// Health is checked lazily because scanning the store can find many environments that will not
/// match the hooks selected for the current command.
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

    fn info(&self) -> Arc<InstallInfo> {
        self.info.clone()
    }

    /// Return the cached install metadata without checking environment health.
    ///
    /// This is used by cache GC, where invalid/unhealthy metadata still describes a directory
    /// that may need to be considered for retention or cleanup.
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

/// Lazy cache of hook environments already present in the store.
///
/// This cache does not track environments created during the current command. New environments
/// are returned by `install_hooks` directly, and same-call reuse happens inside `install_partition`.
pub(crate) struct InstallCache {
    store_hooks: OnceCell<Vec<CachedInstallInfo>>,
}

impl InstallCache {
    /// Create an empty cache; the store is scanned on first access.
    pub(crate) fn new() -> Self {
        Self {
            store_hooks: OnceCell::new(),
        }
    }

    /// Return environments loaded from the store hooks directory.
    ///
    /// Loading is lazy and happens at most once per `InstallCache`. Callers should hold the store
    /// lock while using this in command paths that can race with install or cache cleanup.
    pub(crate) async fn installed_hooks<'a>(
        &'a self,
        store: &Store,
    ) -> impl Iterator<Item = &'a CachedInstallInfo> + 'a {
        let store_hooks = self.store_hooks(store).await;
        store_hooks.iter()
    }

    /// Return a healthy installed environment from the store cache for this hook.
    ///
    /// This only looks at environments loaded from `store.hooks_dir()`. Environments created
    /// during the current install call are reused inside `install_partition`, where hooks in the
    /// same install partition are processed sequentially.
    pub(crate) async fn installed_hook(
        &self,
        store: &Store,
        hook: Arc<Hook>,
    ) -> Option<InstalledHook> {
        let requirement = hook.environment_requirement()?;
        for env in self.installed_hooks(store).await {
            if requirement.is_satisfied_by(env.info_ref()) && env.ensure_healthy().await {
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
                let mut tasks = futures_util::stream::iter(dirs)
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
                    .buffer_unordered(*INTERNAL_CONCURRENCY);

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
}
