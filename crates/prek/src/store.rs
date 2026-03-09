use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use etcetera::BaseStrategy;
use futures::StreamExt;
use rustc_hash::{FxHashMap, FxHashSet};
use thiserror::Error;
use tracing::{debug, warn};

use prek_consts::env_vars::EnvVars;

use crate::config::RemoteRepo;
use crate::fs::LockedFile;
use crate::git::{self, TerminalPrompt};
use crate::hook::InstallInfo;
use crate::run::CONCURRENCY;
use crate::warn_user;
use crate::workspace::{HookInitReporter, WorkspaceCache};

struct PendingClone<'a> {
    repo: &'a RemoteRepo,
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

/// Expand a path starting with `~` to the user's home directory.
fn expand_tilde(path: PathBuf) -> PathBuf {
    if let Ok(stripped) = path.strip_prefix("~") {
        if let Some(home) = std::env::home_dir() {
            return home.join(stripped);
        }
    }
    path
}

pub(crate) const REPO_MARKER: &str = ".prek-repo.json";

/// A store for managing repos.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub(crate) fn from_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Create a store from environment variables or default paths.
    pub(crate) fn from_settings() -> Result<Self, Error> {
        let path = if let Some(path) = EnvVars::var_os(EnvVars::PREK_HOME) {
            Some(expand_tilde(PathBuf::from(path)))
        } else {
            etcetera::choose_base_strategy()
                .map(|path| path.cache_dir().join("prek"))
                .ok()
        };

        let Some(path) = path else {
            return Err(Error::HomeNotFound);
        };
        let store = Store::from_path(path).init()?;

        Ok(store)
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_ref()
    }

    /// Initialize the store.
    pub(crate) fn init(self) -> Result<Self, Error> {
        fs_err::create_dir_all(&self.path)?;
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
        git::clone_repo(&repo.repo, &repo.rev, temp.path(), terminal_prompt).await?;
        Ok(temp)
    }

    async fn persist_cloned_repo(
        &self,
        repo: &RemoteRepo,
        temp: tempfile::TempDir,
    ) -> Result<PathBuf, Error> {
        let target = self.repo_path(repo);

        // TODO: add windows retry
        fs_err::tokio::remove_dir_all(&target).await.ok();
        fs_err::tokio::rename(temp, &target).await?;

        let content = serde_json::to_string_pretty(&repo)?;
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
    ) -> Result<FxHashMap<RemoteRepo, PathBuf>, Error> {
        #[expect(clippy::mutable_key_type)]
        let mut cloned = FxHashMap::default();
        let mut pending = Vec::new();

        for repo in repos {
            let target = self.repo_path(repo);
            if target.join(REPO_MARKER).try_exists()? {
                cloned.insert(repo.clone(), target);
                continue;
            }

            pending.push(PendingClone { repo });
        }

        let mut auth_failed = Vec::new();
        let mut tasks = futures::stream::iter(pending)
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
                            repo = %pending.repo.repo,
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
                        repo: pending.repo.repo.clone(),
                        error: err,
                    }),
                }
            })
            .buffer_unordered(*CONCURRENCY);

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
                    cloned.insert(repo.clone(), path);
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
                    repo: repo.repo.clone(),
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
                repo.repo
            );
            let temp = self
                .clone_repo_to_temp(repo, TerminalPrompt::Enabled)
                .await
                .map_err(|error| Error::CloneRepo {
                    repo: repo.repo.clone(),
                    error,
                })?;
            let path = self.persist_cloned_repo(repo, temp).await?;
            cloned.insert(repo.clone(), path);
        }

        Ok(cloned)
    }

    /// Clone a single remote repository into the store.
    pub(crate) async fn clone_repo(
        &self,
        repo: &RemoteRepo,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<PathBuf, Error> {
        #[expect(clippy::mutable_key_type)]
        let cloned = self.clone_repos(std::iter::once(repo), reporter).await?;
        cloned.get(repo).cloned().ok_or_else(|| Error::CloneRepo {
            repo: repo.repo.clone(),
            error: git::Error::Io(std::io::Error::other("repo was not cloned")),
        })
    }

    /// Returns installed hooks in the store.
    pub(crate) async fn installed_hooks(&self) -> Vec<Arc<InstallInfo>> {
        let Ok(dirs) = fs_err::read_dir(self.hooks_dir()) else {
            return vec![];
        };

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
                Some(info)
            })
            .buffer_unordered(*CONCURRENCY);

        let mut hooks = Vec::new();
        while let Some(hook) = tasks.next().await {
            if let Some(hook) = hook {
                hooks.push(Arc::new(hook));
            }
        }

        hooks
    }

    pub(crate) async fn lock_async(&self) -> Result<LockedFile, std::io::Error> {
        LockedFile::acquire(self.path.join(".lock"), "store").await
    }

    /// Returns the path to where a remote repo would be stored.
    pub(crate) fn repo_path(&self, repo: &RemoteRepo) -> PathBuf {
        self.repos_dir().join(Self::repo_key(repo))
    }

    /// Returns the store key (directory name) for a remote repo.
    pub(crate) fn repo_key(repo: &RemoteRepo) -> String {
        let mut hasher = DefaultHasher::new();
        repo.hash(&mut hasher);
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

    /// Get all tracked config files.
    ///
    /// Seed `config-tracking.json` from the workspace discovery cache if it doesn't exist.
    /// This is a one-time upgrade helper: it only does work when tracking is empty.
    pub(crate) fn tracked_configs(&self) -> Result<FxHashSet<PathBuf>, Error> {
        let tracking_file = self.config_tracking_file();
        match fs_err::read_to_string(&tracking_file) {
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
            Ok(content) => {
                let tracked = serde_json::from_str(&content).unwrap_or_else(|e| {
                    warn!("Failed to parse config tracking file: {e}, resetting");
                    FxHashSet::default()
                });
                return Ok(tracked);
            }
        }

        let cached = WorkspaceCache::cached_config_paths(self);
        if cached.is_empty() {
            return Ok(FxHashSet::default());
        }

        debug!(
            count = cached.len(),
            "Bootstrapping config tracking from workspace cache"
        );
        self.update_tracked_configs(&cached)?;

        Ok(cached)
    }

    /// Track new config files for GC.
    pub(crate) fn track_configs<'a>(
        &self,
        config_paths: impl Iterator<Item = &'a Path>,
    ) -> Result<(), Error> {
        let mut tracked = self.tracked_configs()?;
        for config_path in config_paths {
            tracked.insert(config_path.to_path_buf());
        }

        let tracking_file = self.config_tracking_file();
        let content = serde_json::to_string_pretty(&tracked)?;
        fs_err::write(&tracking_file, content)?;

        Ok(())
    }

    /// Update the tracked configs file.
    pub(crate) fn update_tracked_configs(&self, configs: &FxHashSet<PathBuf>) -> Result<(), Error> {
        let tracking_file = self.config_tracking_file();
        let content = serde_json::to_string_pretty(configs)?;
        fs_err::write(&tracking_file, content)?;

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
}

#[derive(Copy, Clone, Eq, Hash, PartialEq, strum::AsRefStr, strum::Display)]
#[strum(serialize_all = "lowercase")]
pub(crate) enum CacheBucket {
    Uv,
    Go,
    Python,
    Cargo,
    Prek,
}

/// Convert a u64 to a hex string.
fn to_hex(num: u64) -> String {
    hex::encode(num.to_le_bytes())
}
