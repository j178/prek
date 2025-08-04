use std::hash::{DefaultHasher, Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::Result;
use etcetera::BaseStrategy;
use thiserror::Error;
use tracing::debug;

use constants::env_vars::EnvVars;

use crate::config::RemoteRepo;
use crate::fs::LockedFile;
use crate::git::clone_repo;
use crate::hook::InstallInfo;

#[derive(Debug, Error)]
pub enum Error {
    #[error("Home directory not found")]
    HomeNotFound,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Fmt(#[from] std::fmt::Error),
    #[error(transparent)]
    Repo(#[from] crate::hook::Error),
    #[error(transparent)]
    Git(#[from] crate::git::Error),
    #[error(transparent)]
    Serde(#[from] serde_json::Error),
}

static STORE_HOME: LazyLock<Option<PathBuf>> = LazyLock::new(|| {
    if let Some(path) = EnvVars::var_os(EnvVars::PREFLIGIT_HOME) {
        debug!(
            path = %path.to_string_lossy(),
            "Loading store from PREFLIGIT_HOME env var",
        );
        Some(path.into())
    } else {
        etcetera::choose_base_strategy()
            .map(|path| path.cache_dir().join("prefligit"))
            .ok()
    }
});

/// A store for managing repos.
#[derive(Debug)]
pub struct Store {
    path: PathBuf,
}

impl Store {
    pub(crate) fn from_settings() -> Result<Self, Error> {
        Ok(Self::from_path(
            STORE_HOME.as_ref().ok_or(Error::HomeNotFound)?,
        ))
    }

    pub(crate) fn from_path(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_ref()
    }

    /// Initialize the store.
    pub(crate) fn init(self) -> Result<Self, Error> {
        fs_err::create_dir_all(&self.path)?;

        match fs_err::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.path.join("README")) {
            Ok(mut f) => f.write_all(b"This directory is maintained by the prefligit project.\nLearn more: https://github.com/j178/prefligit\n")?,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => (),
            Err(err) => return Err(err.into()),
        }
        Ok(self)
    }

    /// Clone a remote repo into the store.
    pub(crate) async fn clone_repo(&self, repo: &RemoteRepo) -> Result<PathBuf, Error> {
        // Check if the repo is already cloned.
        let target = self.repo_path(repo);
        if target.join(".prefligit-repo.json").try_exists()? {
            return Ok(target);
        }

        fs_err::tokio::create_dir_all(self.repos_dir()).await?;

        // Clone and checkout the repo.
        let temp = tempfile::tempdir_in(self.repos_dir())?;
        debug!(
            target = %temp.path().display(),
            %repo,
            "Cloning repo",
        );
        clone_repo(repo.repo.as_str(), &repo.rev, temp.path()).await?;

        // TODO: add windows retry
        fs_err::tokio::remove_dir_all(&target).await.ok();
        fs_err::tokio::rename(temp, &target).await?;

        let content = serde_json::to_string_pretty(&repo)?;
        fs_err::tokio::write(target.join(".prefligit-repo.json"), content).await?;

        Ok(target)
    }

    /// Returns installed hooks in the store.
    pub(crate) fn installed_hooks(&self) -> impl Iterator<Item = InstallInfo> {
        fs_err::read_dir(self.hooks_dir())
            .ok()
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|entry| {
                let path = entry.path();
                let mut file = fs_err::File::open(path.join(".prefligit-hook.json")).ok()?;
                serde_json::from_reader(&mut file).ok()
            })
    }

    /// Lock the store.
    pub(crate) fn lock(&self) -> Result<LockedFile, std::io::Error> {
        LockedFile::acquire_blocking(self.path.join(".lock"), "store")
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
}

#[derive(Copy, Clone)]
pub(crate) enum ToolBucket {
    Uv,
    Python,
    Node,
    Go,
}

impl ToolBucket {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            ToolBucket::Uv => "uv",
            ToolBucket::Python => "python",
            ToolBucket::Node => "node",
            ToolBucket::Go => "go",
        }
    }
}

#[derive(Copy, Clone)]
pub(crate) enum CacheBucket {
    Uv,
    Go,
}

impl CacheBucket {
    pub(crate) fn as_str(&self) -> &str {
        match self {
            CacheBucket::Uv => "uv",
            CacheBucket::Go => "go",
        }
    }
}

/// Convert a u64 to a hex string.
fn to_hex(num: u64) -> String {
    hex::encode(num.to_le_bytes())
}
