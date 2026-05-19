use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context, Result};
use futures::stream::{FuturesUnordered, StreamExt};
use mea::semaphore::Semaphore;
use rustc_hash::FxHashMap;
use tracing::{debug, warn};

use crate::cli::reporter::HookInstallReporter;
use crate::config::Language;
use crate::hook::{Hook, InstallInfo, InstalledHook};
use crate::run::CONCURRENCY;
use crate::store::Store;

#[derive(Clone, Copy)]
struct InstallPlan {
    /// Language used for install partitioning. This may differ from the runtime language.
    install_language: Language,
    needs_environment: bool,
}

impl InstallPlan {
    fn new(hook: &Hook) -> Self {
        let install_language = if hook.language == Language::Pygrep {
            // Treat `pygrep` hooks as `python` hooks for installation purposes.
            // They share the same installation logic.
            Language::Python
        } else {
            hook.language
        };

        let needs_environment = hook.needs_install_env();

        Self {
            install_language,
            needs_environment,
        }
    }

    fn needs_environment(self) -> bool {
        self.needs_environment
    }
}

/// A hook plus the data that should continue after install resolution.
///
/// `payload` carries the next stage's data through install partitioning without making the install
/// layer know what that stage will do with it.
pub(super) struct InstallJob<T> {
    hook: Arc<Hook>,
    install: InstallPlan,
    payload: T,
}

impl<T> InstallJob<T> {
    pub(super) fn new(hook: Arc<Hook>, payload: T) -> Self {
        let install = InstallPlan::new(&hook);
        Self {
            hook,
            install,
            payload,
        }
    }

    fn hook(&self) -> &Hook {
        self.hook.as_ref()
    }

    fn needs_environment(&self) -> bool {
        self.install.needs_environment()
    }

    fn into_parts(self) -> (Arc<Hook>, InstallPlan, T) {
        (self.hook, self.install, self.payload)
    }
}

pub(crate) async fn install_hooks(
    hooks: Vec<Arc<Hook>>,
    store: &Store,
    reporter: &HookInstallReporter,
) -> Result<Vec<InstalledHook>> {
    let hooks = hooks
        .into_iter()
        .map(|hook| InstallJob::new(hook, ()))
        .collect::<Vec<_>>();
    let num_hooks = hooks.len();
    let mut cache = None;
    let installer = Installer::for_jobs(store, reporter, &mut cache, &hooks).await;
    let result = installer.install_all(hooks).await?;
    reporter.on_complete();

    debug_assert_eq!(
        num_hooks,
        result.len(),
        "Number of hooks installed should match the number of hooks provided"
    );

    Ok(result)
}

#[derive(Clone)]
pub(super) struct Installer<'a> {
    store: &'a Store,
    installed_envs: InstalledEnvs,
    /// Shared by all install partitions for this batch.
    semaphore: Rc<Semaphore>,
    reporter: &'a HookInstallReporter,
}

impl<'a> Installer<'a> {
    pub(super) async fn for_jobs<T>(
        store: &'a Store,
        reporter: &'a HookInstallReporter,
        cache: &mut Option<InstallCache>,
        hooks: &[InstallJob<T>],
    ) -> Self {
        let needs_environment = hooks.iter().any(InstallJob::needs_environment);
        let installed_envs = if needs_environment {
            if let Some(cache) = cache.as_ref() {
                cache.snapshot()
            } else {
                let loaded_cache = InstallCache::load(store).await;
                let installed_envs = loaded_cache.snapshot();
                *cache = Some(loaded_cache);
                installed_envs
            }
        } else {
            InstalledEnvs::empty()
        };

        Self {
            store,
            installed_envs,
            semaphore: Rc::new(Semaphore::new(*CONCURRENCY)),
            reporter,
        }
    }

    async fn install_all(&self, hooks: Vec<InstallJob<()>>) -> Result<Vec<InstalledHook>> {
        let mut result = Vec::with_capacity(hooks.len());
        let mut futures = FuturesUnordered::new();

        for partition in InstallPartitions::new(hooks) {
            let installer = self.partition();
            futures.push(async move { installer.install_all(partition).await });
        }

        while let Some(hooks) = futures.next().await {
            result.extend(hooks?);
        }

        Ok(result)
    }

    pub(super) fn partition(&self) -> PartitionInstaller<'a> {
        PartitionInstaller {
            installer: self.clone(),
            newly_installed: Vec::new(),
        }
    }

    pub(super) fn store(&self) -> &'a Store {
        self.store
    }

    async fn install_new(&self, hook: Arc<Hook>) -> Result<InstalledHook> {
        let _permit = self.semaphore.acquire(1).await;

        let installed_hook = hook
            .language
            .install(hook.clone(), self.store, self.reporter)
            .await
            .with_context(|| format!("Failed to install hook `{hook}`"))?;

        installed_hook
            .mark_as_installed(self.store)
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

    async fn installed_info_for_hook(&self, hook: &Hook) -> Option<Arc<InstallInfo>> {
        for env in self.installed_envs.iter() {
            if env.matches(hook) && env.ensure_healthy().await {
                return Some(env.info());
            }
        }

        None
    }
}

pub(super) struct PartitionInstaller<'a> {
    installer: Installer<'a>,
    /// Environments installed earlier in this partition.
    ///
    /// Partitions contain hooks with the same install language and dependency set, so later hooks
    /// can safely reuse an environment installed by an earlier hook in the same partition.
    newly_installed: Vec<InstalledHook>,
}

impl PartitionInstaller<'_> {
    async fn install_all(mut self, hooks: Vec<InstallJob<()>>) -> Result<Vec<InstalledHook>> {
        let mut installed_hooks = Vec::with_capacity(hooks.len());

        for hook in hooks {
            let (installed_hook, ()) = self.install_job(hook).await?;
            installed_hooks.push(installed_hook);
        }

        Ok(installed_hooks)
    }

    pub(super) async fn install_job<T>(
        &mut self,
        job: InstallJob<T>,
    ) -> Result<(InstalledHook, T)> {
        let (hook, install, payload) = job.into_parts();
        let installed_hook = self.install(hook, install).await?;
        Ok((installed_hook, payload))
    }

    async fn install(&mut self, hook: Arc<Hook>, install: InstallPlan) -> Result<InstalledHook> {
        if !install.needs_environment {
            debug!("Hook `{}` does not need an installed environment", &hook);
            return Ok(InstalledHook::NoNeedInstall(hook));
        }

        if let Some(info) = self.installed_info_for_hook(&hook).await {
            debug!(
                "Found installed environment for hook `{hook}` at `{}`",
                info.env_path.display()
            );
            return Ok(InstalledHook::Installed { hook, info });
        }

        let installed_hook = self.installer.install_new(hook).await?;
        self.newly_installed.push(installed_hook.clone());

        Ok(installed_hook)
    }

    async fn installed_info_for_hook(&self, hook: &Hook) -> Option<Arc<InstallInfo>> {
        for env in &self.newly_installed {
            if let InstalledHook::Installed { info, .. } = env
                && info.matches(hook)
            {
                return Some(info.clone());
            }
        }

        self.installer.installed_info_for_hook(hook).await
    }
}

pub(super) struct InstallPartitions<T> {
    partitions: Vec<Vec<InstallJob<T>>>,
}

impl<T> InstallPartitions<T> {
    /// Group hooks so each partition can install independently.
    ///
    /// Different languages can install concurrently. Hooks with the same language and dependency
    /// set stay in one partition so later hooks can reuse an environment installed by an earlier
    /// hook.
    pub(super) fn new(hooks: Vec<InstallJob<T>>) -> Self {
        let mut hooks_by_language = FxHashMap::default();
        for hook in hooks {
            hooks_by_language
                .entry(hook.install.install_language)
                .or_insert_with(Vec::new)
                .push(hook);
        }

        let mut partitions = Vec::new();
        for (_, hooks) in hooks_by_language {
            partitions.extend(Self::partition_by_dependencies(hooks));
        }

        Self { partitions }
    }

    fn partition_by_dependencies(hooks: Vec<InstallJob<T>>) -> Vec<Vec<InstallJob<T>>> {
        let mut groups: Vec<Vec<InstallJob<T>>> = Vec::new();

        for hook in hooks {
            let group_index = groups.iter().position(|group| {
                group[0].hook().env_key_dependencies() == hook.hook().env_key_dependencies()
            });

            if let Some(index) = group_index {
                groups[index].push(hook);
            } else {
                groups.push(vec![hook]);
            }
        }

        groups
    }
}

impl<T> IntoIterator for InstallPartitions<T> {
    type Item = Vec<InstallJob<T>>;
    type IntoIter = std::vec::IntoIter<Vec<InstallJob<T>>>;

    fn into_iter(self) -> Self::IntoIter {
        self.partitions.into_iter()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct CachedInstallInfo {
    info: Arc<InstallInfo>,
    health: mea::once::OnceCell<bool>,
}

impl CachedInstallInfo {
    fn new(info: Arc<InstallInfo>) -> Self {
        Self {
            info,
            health: mea::once::OnceCell::new(),
        }
    }

    fn healthy(info: Arc<InstallInfo>) -> Self {
        Self {
            info,
            health: mea::once::OnceCell::from_value(true),
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
    store_hooks: Rc<[CachedInstallInfo]>,
    /// Environments installed after the scan and therefore absent from `store_hooks`.
    created_hooks: Vec<CachedInstallInfo>,
}

impl InstallCache {
    pub(crate) async fn load(store: &Store) -> Self {
        Self {
            store_hooks: Self::load_store_installed_hooks(store).await,
            created_hooks: Vec::new(),
        }
    }

    pub(crate) fn installed_hooks(&self) -> impl Iterator<Item = &CachedInstallInfo> {
        self.created_hooks.iter().chain(self.store_hooks.iter())
    }

    async fn load_store_installed_hooks(store: &Store) -> Rc<[CachedInstallInfo]> {
        let store_installed_hooks = match fs_err::read_dir(store.hooks_dir()) {
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
        };
        Rc::from(store_installed_hooks.into_boxed_slice())
    }

    fn snapshot(&self) -> InstalledEnvs {
        InstalledEnvs::new(
            Rc::clone(&self.store_hooks),
            Rc::from(self.created_hooks.clone().into_boxed_slice()),
        )
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
            || self
                .store_hooks
                .iter()
                .any(|existing| existing.info.env_path == info.env_path)
    }
}

/// Installed environments visible to one installer batch.
///
/// This is intentionally a snapshot: installers can run concurrently without borrowing the mutable
/// run-level cache. Newly installed environments are merged back into that cache after the batch.
#[derive(Clone)]
struct InstalledEnvs {
    store_hooks: Rc<[CachedInstallInfo]>,
    created_hooks: Rc<[CachedInstallInfo]>,
}

impl InstalledEnvs {
    fn new(store_hooks: Rc<[CachedInstallInfo]>, created_hooks: Rc<[CachedInstallInfo]>) -> Self {
        Self {
            store_hooks,
            created_hooks,
        }
    }

    fn empty() -> Self {
        Self {
            store_hooks: Rc::from(Vec::new().into_boxed_slice()),
            created_hooks: Rc::from(Vec::new().into_boxed_slice()),
        }
    }

    fn iter(&self) -> impl Iterator<Item = &CachedInstallInfo> {
        self.created_hooks.iter().chain(self.store_hooks.iter())
    }
}
