use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use etcetera::BaseStrategy;
use futures::StreamExt;
use rustc_hash::FxHashSet;
use thiserror::Error;
use tracing::{debug, warn};

use prek_consts::env_vars::EnvVars;

use crate::config::RemoteRepo;
use crate::fs::LockedFile;
use crate::git::clone_repo;
use crate::hook::InstallInfo;
use crate::run::CONCURRENCY;
use crate::workspace::{HookInitReporter, WorkspaceCache};

#[derive(Debug, Error)]
pub enum Error {
    #[error("Home directory not found")]
    HomeNotFound,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Git(#[from] crate::git::Error),
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

    /// Clone a remote repo into the store.
    pub(crate) async fn clone_repo(
        &self,
        repo: &RemoteRepo,
        reporter: Option<&dyn HookInitReporter>,
    ) -> Result<PathBuf, Error> {
        // Check if the repo is already cloned.
        let target = self.repo_path(repo);
        if target.join(REPO_MARKER).try_exists()? {
            return Ok(target);
        }

        let progress =
            reporter.map(|reporter| (reporter, reporter.on_clone_start(&format!("{repo}"))));

        // Clone and checkout the repo.
        let temp = tempfile::tempdir_in(self.scratch_path())?;

        debug!(
            target = %temp.path().display(),
            %repo,
            "Cloning repo",
        );
        clone_repo(&repo.repo, &repo.rev, temp.path()).await?;

        // TODO: add windows retry
        fs_err::tokio::remove_dir_all(&target).await.ok();
        fs_err::tokio::rename(temp, &target).await?;

        let content = serde_json::to_string_pretty(&repo)?;
        fs_err::tokio::write(target.join(REPO_MARKER), content).await?;

        if let Some((reporter, progress)) = progress {
            reporter.on_clone_complete(progress);
        }

        Ok(target)
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
        self.tools_dir().join(tool.as_str())
    }

    pub(crate) fn cache_path(&self, tool: CacheBucket) -> PathBuf {
        self.cache_dir().join(tool.as_str())
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

#[derive(Copy, Clone, Eq, Hash, PartialEq, clap::ValueEnum)]
pub(crate) enum ToolBucket {
    Uv,
    Python,
    Node,
    Go,
    Ruby,
    Rustup,
    Bun,
}

impl ToolBucket {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            ToolBucket::Bun => "bun",
            ToolBucket::Go => "go",
            ToolBucket::Node => "node",
            ToolBucket::Python => "python",
            ToolBucket::Ruby => "ruby",
            ToolBucket::Rustup => "rustup",
            ToolBucket::Uv => "uv",
        }
    }
}

#[derive(Copy, Clone, Eq, Hash, PartialEq)]
pub(crate) enum CacheBucket {
    Uv,
    Go,
    Python,
    Cargo,
    Prek,
}

impl CacheBucket {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            CacheBucket::Go => "go",
            CacheBucket::Prek => "prek",
            CacheBucket::Python => "python",
            CacheBucket::Cargo => "cargo",
            CacheBucket::Uv => "uv",
        }
    }
}

/// Convert a u64 to a hex string.
fn to_hex(num: u64) -> String {
    hex::encode(num.to_le_bytes())
}
