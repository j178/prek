use std::hash::{Hash, Hasher};
use std::io::Write;
use std::ops::Deref;
use std::path::{Path, PathBuf};

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
use crate::fs::{LockedFile, expand_tilde, open_lock_file};
use crate::git::{self, TerminalPrompt};
use crate::warn_user;
use crate::workspace::HookInitReporter;

struct PendingClone<'a> {
    repo: &'a RemoteRepo,
}

#[derive(serde::Serialize)]
struct RepoMarker<'a> {
    repo: &'a str,
    rev: &'a str,
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
    fn from_lock(err: std::fs::TryLockError, path: &Path) -> Self {
        match err {
            std::fs::TryLockError::WouldBlock => Self::CacheInUse(path.to_owned()),
            std::fs::TryLockError::Error(err) => Self::Io(err),
        }
    }
}

pub(crate) const REPO_MARKER: &str = ".prek-repo.json";
pub(crate) const STORE_LOCK: &str = ".lock";

#[derive(Debug, Clone, Copy)]
pub(crate) enum CacheLock {
    Shared,
    TryShared,
    TryExclusive,
}

/// Holds the cache lock until this command finishes using its files.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
    mode: CacheLock,
    _cache_lock: fs_err::File,
}

impl Store {
    /// Create a store from environment variables or default paths.
    pub(crate) fn from_settings(mode: CacheLock) -> Result<Self, Error> {
        let path = if let Some(path) = EnvVars.var_os(EnvVars::PREK_HOME) {
            Some(expand_tilde(PathBuf::from(path)))
        } else {
            etcetera::choose_base_strategy()
                .map(|path| path.cache_dir().join("prek"))
                .ok()
        };

        let Some(path) = path else {
            return Err(Error::HomeNotFound);
        };
        Self::open(&path, mode)
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_ref()
    }

    pub(crate) fn from_path(path: impl AsRef<Path>) -> Result<Self, Error> {
        Self::open(path.as_ref(), CacheLock::Shared)
    }

    fn open(path: &Path, mode: CacheLock) -> Result<Self, Error> {
        let path = std::path::absolute(path)?;
        fs_err::create_dir_all(&path)?;
        let lock = open_lock_file(&path.join(STORE_LOCK))?;
        // A hook or installer can invoke a cache command while its parent holds a lock.
        match mode {
            CacheLock::Shared => lock.lock_shared()?,
            CacheLock::TryShared => lock
                .try_lock_shared()
                .map_err(|e| Error::from_lock(e, &path))?,
            CacheLock::TryExclusive => lock.try_lock().map_err(|e| Error::from_lock(e, &path))?,
        }
        let store = Self {
            path,
            mode,
            _cache_lock: lock,
        };
        // Cleanup must also work on an empty or partially initialized cache.
        if !matches!(mode, CacheLock::Shared) {
            return Ok(store);
        }
        fs_err::create_dir_all(store.repos_dir())?;
        fs_err::create_dir_all(store.hooks_dir())?;
        fs_err::create_dir_all(store.scratch_path())?;

        match fs_err::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(store.path.join("README")) {
            Ok(mut f) => f.write_all(b"This directory is maintained by the prek project.\nLearn more: https://github.com/j178/prek\n")?,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(err) => return Err(err.into()),
        }
        Ok(store)
    }

    /// Serialize repository, environment, and config tracking updates by concurrent cache users.
    pub(crate) async fn lock(&self) -> Result<StoreGuard<'_>, Error> {
        let path = self.path.join(".write.lock");
        let write_lock = match self.mode {
            CacheLock::Shared => Some(LockedFile::acquire(path, "store").await?),
            CacheLock::TryShared => {
                Some(LockedFile::try_acquire(&path).map_err(|e| Error::from_lock(e, &self.path))?)
            }
            // Exclusive access already excludes every writer and everyone waiting to write.
            CacheLock::TryExclusive => None,
        };
        Ok(StoreGuard {
            store: self,
            _write_lock: write_lock,
        })
    }

    /// Returns the path to where a remote repo would be stored.
    pub(crate) fn repo_path(&self, repo: &RemoteRepo) -> PathBuf {
        self.repos_dir().join(Self::repo_key(repo))
    }

    /// Returns the store key (directory name) for a remote repo.
    pub(crate) fn repo_key(repo: &RemoteRepo) -> String {
        let mut hasher = SeaHasher::new();
        repo.source().hash(&mut hasher);
        repo.rev.hash(&mut hasher);
        to_hex(hasher.finish())
    }

    pub(crate) fn repos_dir(&self) -> PathBuf {
        self.path.join("repos")
    }

    pub(crate) fn hooks_dir(&self) -> PathBuf {
        self.path.join("hooks")
    }

    pub(crate) fn patches_dir(&self) -> PathBuf {
        self.path.join("patches")
    }

    pub(crate) fn tools_dir(&self) -> PathBuf {
        self.path.join("tools")
    }

    pub(crate) fn cache_dir(&self) -> PathBuf {
        self.path.join("cache")
    }

    /// The path to the tool directory in the store.
    pub(crate) fn tools_path(&self, tool: ToolBucket) -> PathBuf {
        self.tools_dir().join(tool.as_ref())
    }

    pub(crate) fn cache_path(&self, tool: CacheBucket) -> PathBuf {
        self.cache_dir().join(tool.as_ref())
    }

    /// Scratch path for temporary files.
    pub(crate) fn scratch_path(&self) -> PathBuf {
        self.path.join("scratch")
    }

    pub(crate) fn log_file(&self) -> PathBuf {
        self.path.join("prek.log")
    }

    pub(crate) fn config_tracking_file(&self) -> PathBuf {
        self.path.join("config-tracking.json")
    }
}

/// Exclusive access to repository, environment, and config tracking updates.
#[derive(Debug)]
#[must_use = "the store lock is released when the guard is dropped"]
pub(crate) struct StoreGuard<'a> {
    store: &'a Store,
    _write_lock: Option<LockedFile>,
}

impl Deref for StoreGuard<'_> {
    type Target = Store;

    fn deref(&self) -> &Self::Target {
        self.store
    }
}

impl StoreGuard<'_> {
    async fn clone_repo_to_temp(
        &self,
        repo: &RemoteRepo,
        terminal_prompt: TerminalPrompt,
    ) -> Result<tempfile::TempDir, git::Error> {
        let temp = tempfile::tempdir_in(self.scratch_path())?;
        debug!(
            target = %temp.path().display(),
            %repo,
            ?terminal_prompt,
            "Cloning repo"
        );
        git::clone_repo(repo.source(), &repo.rev, temp.path(), terminal_prompt).await?;
        Ok(temp)
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
            repo: repo.source(),
            rev: &repo.rev,
        };
        let content = serde_json::to_string_pretty(&marker)?;
        fs_err::tokio::write(target.join(REPO_MARKER), content).await?;

        Ok(target)
    }

    /// Clone remote repositories into the store.
    ///
    /// The first pass runs in parallel with terminal prompts disabled. Repositories that fail
    /// with an authentication error are retried afterwards, sequentially, with terminal prompts
    /// enabled so the user can provide credentials for one repository at a time.
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

            pending.push(PendingClone { repo });
        }

        let mut auth_failed = Vec::new();
        let mut tasks = futures_util::stream::iter(pending)
            .map(async |pending| {
                let progress =
                    reporter.map(|reporter| reporter.on_clone_start(&format!("{}", pending.repo)));
                match self
                    .clone_repo_to_temp(pending.repo, TerminalPrompt::Disabled)
                    .await
                {
                    Ok(temp) => Ok(FirstClonePass::Ready {
                        repo: pending.repo,
                        temp,
                        progress,
                    }),
                    Err(err) if git::is_auth_error(&err) => {
                        warn!(
                            repo = %pending.repo.repo(),
                            ?err,
                            "Clone failed with authentication error and terminal prompts disabled"
                        );
                        Ok(FirstClonePass::AuthFailed {
                            repo: pending.repo,
                            error: err,
                            progress,
                        })
                    }
                    Err(err) => Err(Error::CloneRepo {
                        repo: pending.repo.repo().to_string(),
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
                .clone_repo_to_temp(repo, TerminalPrompt::Enabled)
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

    /// Clone a single remote repository into the store.
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

    /// Get all tracked config files.
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

    /// Track new config files for GC.
    pub(crate) fn track_configs<'a>(
        &self,
        config_paths: impl Iterator<Item = &'a Path>,
    ) -> Result<(), Error> {
        let mut tracked = self.tracked_configs()?;
        let previous_len = tracked.len();
        tracked.extend(config_paths.map(Path::to_path_buf));

        if tracked.len() != previous_len {
            self.update_tracked_configs(&tracked)?;
        }

        Ok(())
    }

    /// Update the tracked configs file.
    pub(crate) fn update_tracked_configs(&self, configs: &FxHashSet<PathBuf>) -> Result<(), Error> {
        let content = serde_json::to_string_pretty(configs)?;
        fs_err::write(self.config_tracking_file(), content)?;

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
