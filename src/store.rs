use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::Result;
use etcetera::BaseStrategy;
use futures::StreamExt;
use thiserror::Error;
use tracing::{debug, warn};

use prek_consts::env_vars::EnvVars;

use crate::config::RemoteRepo;
use crate::fs::LockedFile;
use crate::git::clone_repo;
use crate::hook::InstallInfo;
use crate::run::CONCURRENCY;
use crate::workspace::HookInitReporter;

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
            Some(path.into())
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
        if target.join(".prek-repo.json").try_exists()? {
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
        fs_err::tokio::write(target.join(".prek-repo.json"), content).await?;

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

    /// Returns the path to the cloned repo.
    fn repo_path(&self, repo: &RemoteRepo) -> PathBuf {
        let mut hasher = DefaultHasher::new();
        repo.hash(&mut hasher);
        let digest = to_hex(hasher.finish());
        self.repos_dir().join(digest)
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

    /// The path to the tool directory in the store.
    pub(crate) fn tools_path(&self, tool: ToolBucket) -> PathBuf {
        self.path.join("tools").join(tool.as_str())
    }

    pub(crate) fn cache_path(&self, tool: CacheBucket) -> PathBuf {
        self.path.join("cache").join(tool.as_str())
    }

    /// Scratch path for temporary files.
    pub(crate) fn scratch_path(&self) -> PathBuf {
        self.path.join("scratch")
    }

    pub(crate) fn log_file(&self) -> PathBuf {
        self.path.join("prek.log")
    }
}

#[derive(Copy, Clone)]
pub(crate) enum ToolBucket {
    Uv,
    Python,
    Node,
    Go,
    Ruby,
    Rustup,
}

impl ToolBucket {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            ToolBucket::Go => "go",
            ToolBucket::Node => "node",
            ToolBucket::Python => "python",
            ToolBucket::Ruby => "ruby",
            ToolBucket::Rustup => "rustup",
            ToolBucket::Uv => "uv",
        }
    }
}

#[derive(Copy, Clone)]
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
