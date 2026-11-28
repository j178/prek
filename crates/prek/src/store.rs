use std::hash::{Hash, Hasher};
use std::io::Write;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use anyhow::Result;
use etcetera::BaseStrategy;
use futures_util::StreamExt;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use rustc_hash::{FxHashMap, FxHashSet};
use seahash::SeaHasher;
use thiserror::Error;
use tracing::{debug, warn};

use crate::concurrency::INTERNAL_CONCURRENCY;
use crate::config::{RemoteRepo, RemoteRepoKey};
use crate::fs::{LockedFile, expand_tilde};
use crate::git::{self, TerminalPrompt};
use crate::warn_user;
use crate::workspace::HookInitReporter;

struct CachedRevision {
    path: PathBuf,
    installed_at: SystemTime,
}

#[derive(serde::Deserialize, serde::Serialize)]
pub(crate) struct RepoMarker {
    pub(crate) repo: String,
    pub(crate) rev: String,
}

impl RepoMarker {
    pub(crate) fn read(root: &Path) -> Option<Self> {
        let content = fs_err::read(root.join(REPO_MARKER)).ok()?;
        serde_json::from_slice(&content).ok()
    }
}

enum FirstClonePass<'a> {
    Ready {
        repo: &'a RemoteRepo,
        temp: tempfile::TempDir,
        progress: Option<usize>,
    },
    AuthFailed {
        repo: &'a RemoteRepo,
        error: git::Error,
        progress: Option<usize>,
    },
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("Home directory not found")]
    HomeNotFound,
    #[error("Cache at `{0}` is in use. Retry after other prek commands finish.")]
    CacheInUse(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error("Failed to clone repo `{repo}`")]
    CloneRepo {
        repo: String,
        #[source]
        error: git::Error,
    },
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

impl Error {
    fn from_lock(err: std::io::Error, path: &Path) -> Self {
        if err.kind() == std::io::ErrorKind::WouldBlock {
            Self::CacheInUse(path.to_owned())
        } else {
            Self::Io(err)
        }
    }
}

pub(crate) const REPO_MARKER: &str = ".prek-repo.json";
pub(crate) const STORE_LOCK: &str = ".lock";
const STORE_WRITE_LOCK: &str = ".write.lock";

/// The on-disk cache for hook repositories, environments, and toolchains.
///
/// `PREK_HOME` overrides the default `prek` subdirectory of the platform cache
/// directory. Opening a store creates the root and lock file as needed.
/// The store holds its cache lock until it is dropped.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    _cache_lock: LockedFile,
}

impl Store {
    /// Open the configured cache, waiting until a shared lock is available.
    /// Call [`Self::init`] before preparing hooks.
    pub(crate) fn open() -> Result<Self, Error> {
        Self::open_configured(LockedFile::acquire_shared)
    }

    /// Open the configured cache with a shared lock. Return [`Error::CacheInUse`]
    /// immediately if the cache is locked exclusively.
    pub(crate) fn try_open() -> Result<Self, Error> {
        Self::open_configured(LockedFile::try_shared)
    }

    /// Open the configured cache with an exclusive lock. Return [`Error::CacheInUse`]
    /// immediately if another handle holds a shared or exclusive lock.
    pub(crate) fn try_open_exclusive() -> Result<ExclusiveStore, Error> {
        Self::open_configured(LockedFile::try_exclusive).map(ExclusiveStore)
    }

    fn open_configured(lock: fn(&Path) -> std::io::Result<LockedFile>) -> Result<Self, Error> {
        let path = if let Some(path) = EnvVars.var_os(EnvVars::PREK_HOME) {
            expand_tilde(PathBuf::from(path))
        } else {
            etcetera::choose_base_strategy()
                .map_err(|_| Error::HomeNotFound)?
                .cache_dir()
                .join("prek")
        };
        Self::open_at(&path, lock)
    }

    /// The absolute path to the cache root.
    pub(crate) fn path(&self) -> &Path {
        self.path.as_ref()
    }

    fn open_at(path: &Path, lock: fn(&Path) -> std::io::Result<LockedFile>) -> Result<Self, Error> {
        let path = std::path::absolute(path)?;
        fs_err::create_dir_all(&path)?;
        let cache_lock =
            lock(&path.join(STORE_LOCK)).map_err(|err| Error::from_lock(err, &path))?;
        Ok(Self {
            path,
            _cache_lock: cache_lock,
        })
    }

    /// Open and initialize a cache at `path`, waiting for shared access.
    pub(crate) fn init_at(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open_at(path.as_ref(), LockedFile::acquire_shared)?.init()
    }

    /// Ensure the repository, hook environment, and scratch directories exist.
    /// Create the cache README if it is missing.
    pub(crate) fn init(self) -> Result<Self, Error> {
        fs_err::create_dir_all(self.repos_dir())?;
        fs_err::create_dir_all(self.hooks_dir())?;
        fs_err::create_dir_all(self.scratch_path())?;

        match fs_err::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path.join("README")) {
            Ok(mut f) => f.write_all(b"This directory is maintained by the prek project.\nLearn more: https://github.com/j178/prek\n")?,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(err) => return Err(err.into()),
        }
        Ok(self)
    }

    /// Wait for the write lock before preparing hooks or updating tracked configs.
    /// Drop the guard before executing a hook so a nested `prek run` can acquire it.
    pub(crate) async fn write_lock(&self) -> Result<StoreWriteGuard<'_>, Error> {
        let lock = LockedFile::acquire(self.path.join(STORE_WRITE_LOCK), "store updates").await?;
        Ok(StoreWriteGuard {
            store: self,
            _write_lock: lock,
        })
    }

    /// Acquire the same lock as [`Self::write_lock`], returning [`Error::CacheInUse`]
    /// immediately if another writer holds it.
    pub(crate) fn try_write_lock(&self) -> Result<StoreWriteGuard<'_>, Error> {
        let lock = LockedFile::try_exclusive(&self.path.join(STORE_WRITE_LOCK))
            .map_err(|err| Error::from_lock(err, &self.path))?;
        Ok(StoreWriteGuard {
            store: self,
            _write_lock: lock,
        })
    }

    /// The checkout path for this repository and revision, whether or not it exists yet.
    pub(crate) fn repo_path(&self, repo: &RemoteRepo) -> PathBuf {
        self.repos_dir()
            .join(Self::repo_key(repo.source(), &repo.rev))
    }

    pub(crate) fn repo_key(source: &str, rev: &str) -> String {
        let mut hasher = SeaHasher::new();
        source.hash(&mut hasher);
        rev.hash(&mut hasher);
        to_hex(hasher.finish())
    }

    /// Directory containing cached repository checkouts.
    pub(crate) fn repos_dir(&self) -> PathBuf {
        self.path.join("repos")
    }

    /// Git objects and refs retained between update checks, grouped by source.
    pub(crate) fn updates_dir(&self) -> PathBuf {
        self.path.join("updates")
    }

    /// Directory containing installed hook environments.
    pub(crate) fn hooks_dir(&self) -> PathBuf {
        self.path.join("hooks")
    }

    /// Directory for patches that preserve unstaged changes during a hook run.
    pub(crate) fn patches_dir(&self) -> PathBuf {
        self.path.join("patches")
    }

    /// Root directory for toolchains installed by prek.
    pub(crate) fn tools_dir(&self) -> PathBuf {
        self.path.join("tools")
    }

    /// Root directory for tool caches and prek's own cached metadata.
    pub(crate) fn cache_dir(&self) -> PathBuf {
        self.path.join("cache")
    }

    /// Directory for installed versions of a particular tool.
    pub(crate) fn tools_path(&self, tool: ToolBucket) -> PathBuf {
        self.tools_dir().join(tool.as_ref())
    }

    /// Cache directory for a single tool or for prek itself.
    pub(crate) fn cache_path(&self, tool: CacheBucket) -> PathBuf {
        self.cache_dir().join(tool.as_ref())
    }

    /// Workspace for temporary files such as in-progress clones and downloads.
    pub(crate) fn scratch_path(&self) -> PathBuf {
        self.path.join("scratch")
    }

    /// Default log path for commands that write a trace log.
    pub(crate) fn log_file(&self) -> PathBuf {
        self.path.join("prek.log")
    }

    /// File recording the config paths that garbage collection consults.
    pub(crate) fn config_tracking_file(&self) -> PathBuf {
        self.path.join("config-tracking.json")
    }

    /// Read the config paths recorded for garbage collection. Hold the write
    /// lock or the exclusive cache lock for a consistent read. A missing file
    /// or JSON that cannot be decoded as a set of paths yields an empty set.
    pub(crate) fn tracked_configs(&self) -> Result<FxHashSet<PathBuf>, Error> {
        let tracking_file = self.config_tracking_file();
        match fs_err::read_to_string(&tracking_file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(FxHashSet::default()),
            Err(e) => Err(e.into()),
            Ok(content) => {
                let tracked = serde_json::from_str(&content).unwrap_or_else(|e| {
                    warn!("Failed to parse config tracking file: {e}, resetting");
                    FxHashSet::default()
                });
                Ok(tracked)
            }
        }
    }

    fn write_tracked_configs(&self, configs: &FxHashSet<PathBuf>) -> Result<(), Error> {
        let content = serde_json::to_string_pretty(configs)?;
        fs_err::write(self.config_tracking_file(), content)?;

        Ok(())
    }
}

/// Allows cache entries to be removed. Other commands cannot open this cache
/// until the exclusive store is dropped.
pub(crate) struct ExclusiveStore(Store);

impl Deref for ExclusiveStore {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl ExclusiveStore {
    /// Replace the tracked config paths with the set retained by garbage collection.
    pub(crate) fn update_tracked_configs(&self, configs: &FxHashSet<PathBuf>) -> Result<(), Error> {
        self.write_tracked_configs(configs)
    }
}

/// Serializes checkout creation, hook installation, and config tracking updates.
/// Dropping the guard releases the write lock. The store's cache lock stays
/// held, so other commands can prepare hooks while this one runs.
#[derive(Debug)]
#[must_use]
pub(crate) struct StoreWriteGuard<'a> {
    store: &'a Store,
    _write_lock: LockedFile,
}

impl Deref for StoreWriteGuard<'_> {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl StoreWriteGuard<'_> {
    /// Pick the most recently installed revision of each missing repository.
    fn cached_revisions(&self, pending: &[&RemoteRepo]) -> FxHashMap<String, CachedRevision> {
        let mut newest: FxHashMap<String, CachedRevision> = FxHashMap::default();
        let Ok(entries) = fs_err::read_dir(self.repos_dir()) else {
            return newest;
        };

        let sources: FxHashSet<_> = pending.iter().map(|repo| repo.source()).collect();
        for entry in entries.flatten() {
            let path = entry.path();
            let Some(marker) = RepoMarker::read(&path) else {
                continue;
            };
            if !sources.contains(marker.repo.as_str()) {
                continue;
            }

            // A hook can modify its checkout. Only trust a marker that still
            // identifies the cache directory where prek installed it.
            let key = Store::repo_key(&marker.repo, &marker.rev);
            if entry.file_name() != key.as_str() {
                continue;
            }

            let Ok(installed_at) =
                fs_err::metadata(path.join(REPO_MARKER)).and_then(|m| m.modified())
            else {
                continue;
            };
            if newest
                .get(&marker.repo)
                .is_none_or(|prev| prev.installed_at < installed_at)
            {
                newest.insert(marker.repo, CachedRevision { path, installed_at });
            }
        }

        newest
    }

    async fn clone_repo_to_temp(
        &self,
        repo: &RemoteRepo,
        terminal_prompt: TerminalPrompt,
        cached: Option<&Path>,
    ) -> Result<tempfile::TempDir, git::Error> {
        debug!(%repo, ?terminal_prompt, ?cached, "Cloning repo");
        git::clone_repo(
            repo.source(),
            &repo.rev,
            &self.scratch_path(),
            terminal_prompt,
            cached,
        )
        .await
    }

    async fn persist_cloned_repo(
        &self,
        repo: &RemoteRepo,
        temp: tempfile::TempDir,
    ) -> Result<PathBuf, Error> {
        let target = self.repo_path(repo);

        fs_err::tokio::remove_dir_all(&target).await.ok();
        crate::fs::rename_with_retry(temp, &target).await?;

        let marker = RepoMarker {
            repo: repo.source().to_owned(),
            rev: repo.rev.clone(),
        };
        let content = serde_json::to_string_pretty(&marker)?;
        fs_err::tokio::write(target.join(REPO_MARKER), content).await?;

        Ok(target)
    }

    /// Return a checkout path for each requested repository and revision,
    /// reusing cached checkouts and cloning those that are missing.
    ///
    /// Clones run in parallel with terminal prompts disabled. Outside CI,
    /// authentication failures are retried one at a time with prompts enabled.
    pub(crate) async fn clone_repos<'a>(
        &self,
        repos: impl IntoIterator<Item = &'a RemoteRepo>,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<FxHashMap<RemoteRepoKey<'a>, PathBuf>, Error> {
        let mut cloned = FxHashMap::default();
        let mut pending = Vec::new();

        for repo in repos {
            let target = self.repo_path(repo);
            if target.join(REPO_MARKER).try_exists()? {
                cloned.insert(repo.key(), target);
                continue;
            }

            pending.push(repo);
        }

        if pending.is_empty() {
            return Ok(cloned);
        }

        let cached = self.cached_revisions(&pending);
        let mut auth_failed = Vec::new();
        let mut tasks = futures_util::stream::iter(pending)
            .map(async |repo| {
                let progress = reporter.map(|reporter| reporter.on_clone_start(&repo.to_string()));
                let cached_path = cached.get(repo.source()).map(|rev| rev.path.as_path());
                let result = self
                    .clone_repo_to_temp(repo, TerminalPrompt::Disabled, cached_path)
                    .await;

                match result {
                    Ok(temp) => Ok(FirstClonePass::Ready {
                        repo,
                        temp,
                        progress,
                    }),
                    Err(err) if git::is_auth_error(&err) => {
                        warn!(
                            repo = %repo.repo(),
                            ?err,
                            "Clone failed with authentication error and terminal prompts disabled"
                        );
                        Ok(FirstClonePass::AuthFailed {
                            repo,
                            error: err,
                            progress,
                        })
                    }
                    Err(err) => Err(Error::CloneRepo {
                        repo: repo.repo().to_string(),
                        error: err,
                    }),
                }
            })
            .buffer_unordered(*INTERNAL_CONCURRENCY);

        while let Some(result) = tasks.next().await {
            match result? {
                FirstClonePass::Ready {
                    repo,
                    temp,
                    progress,
                } => {
                    let path = self.persist_cloned_repo(repo, temp).await?;
                    if let (Some(reporter), Some(progress)) = (reporter, progress) {
                        reporter.on_clone_complete(progress);
                    }
                    cloned.insert(repo.key(), path);
                }
                FirstClonePass::AuthFailed {
                    repo,
                    error,
                    progress,
                } => {
                    if let (Some(reporter), Some(progress)) = (reporter, progress) {
                        reporter.on_clone_complete(progress);
                    }
                    auth_failed.push((repo, error));
                }
            }
        }

        if EnvVars::is_under_ci() {
            // CI cannot answer interactive credential prompts, so surface the original auth
            // failure instead of attempting the prompt-enabled retry path.
            if let Some((repo, error)) = auth_failed.into_iter().next() {
                return Err(Error::CloneRepo {
                    repo: repo.repo().to_string(),
                    error,
                });
            }

            return Ok(cloned);
        }

        if !auth_failed.is_empty() {
            // Tear down the shared MultiProgress before warning/prompt output so progress redraws
            // do not overwrite terminal messages or git credential prompts.
            reporter.map(HookInitReporter::on_complete);
        }

        for (repo, _error) in auth_failed {
            warn_user!(
                "Authentication may be required to clone repository `{}`. Retrying with terminal prompts enabled.",
                repo.repo()
            );
            let temp = self
                .clone_repo_to_temp(repo, TerminalPrompt::Enabled, None)
                .await
                .map_err(|error| Error::CloneRepo {
                    repo: repo.repo().to_string(),
                    error,
                })?;
            let path = self.persist_cloned_repo(repo, temp).await?;
            cloned.insert(repo.key(), path);
        }

        Ok(cloned)
    }

    /// Return the local checkout for this repository and revision, cloning it if needed.
    pub(crate) async fn clone_repo(
        &self,
        repo: &RemoteRepo,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<PathBuf, Error> {
        let repo_key = repo.key();
        let cloned = self.clone_repos(std::iter::once(repo), reporter).await?;
        cloned
            .get(&repo_key)
            .cloned()
            .ok_or_else(|| Error::CloneRepo {
                repo: repo.repo().to_string(),
                error: git::Error::Io(std::io::Error::other("repo was not cloned")),
            })
    }

    /// Add config paths to the set that garbage collection uses to find live cache entries.
    pub(crate) fn track_configs<'a>(
        &self,
        config_paths: impl Iterator<Item = &'a Path>,
    ) -> Result<(), Error> {
        let mut tracked = self.tracked_configs()?;
        let previous_len = tracked.len();
        tracked.extend(config_paths.map(Path::to_path_buf));

        if tracked.len() != previous_len {
            self.write_tracked_configs(&tracked)?;
        }

        Ok(())
    }
}

#[derive(Copy, Clone, Eq, Hash, PartialEq, strum::EnumIter, strum::AsRefStr, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum ToolBucket {
    Uv,
    Python,
    Node,
    Go,
    Ruby,
    Rustup,
    Bun,
    Dotnet,
    Deno,
    Mise,
}

#[derive(Copy, Clone, Eq, Hash, PartialEq, strum::AsRefStr, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum CacheBucket {
    Uv,
    Go,
    Python,
    Cargo,
    Deno,
    Npm,
    Coursier,
    Prek,
}

/// Convert a u64 to a hex string.
fn to_hex(num: u64) -> String {
    hex::encode(num.to_le_bytes())
}
