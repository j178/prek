use std::hash::{Hash, Hasher};
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use etcetera::BaseStrategy;
use futures_util::StreamExt;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
use rustc_hash::{FxHashMap, FxHashSet};
use seahash::SeaHasher;
use thiserror::Error;
use tracing::{debug, warn};

use crate::config::{RemoteRepo, RemoteRepoKey};
use crate::fs::{LockedFile, expand_tilde};
use crate::git::{self, TerminalPrompt};
use crate::run::INTERNAL_CONCURRENCY;
use crate::warn_user;
use crate::workspace::{HookInitReporter, WorkspaceCache};

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
        let store = Store::from_path(path).init()?;

        Ok(store)
    }

    pub(crate) fn path(&self) -> &Path {
        self.path.as_ref()
    }

    /// Initialize the store.
    pub(crate) fn init(self) -> Result<Self, Error> {
        fs_err::create_dir_all(&self.path)?;
        fs_err::create_dir_all(self.repo_sources_dir())?;
        fs_err::create_dir_all(self.repo_source_locks_dir())?;
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
        let source = self.repo_source_path(repo.source());
        let _source_lock = self.repo_source_lock(repo.source()).await?;
        debug!(
            source = %source.display(),
            target = %temp.path().display(),
            %repo,
            ?terminal_prompt,
            "Preparing repo checkout"
        );
        git::ensure_bare_repo(repo.source(), &source).await?;
        let revision = git::fetch_repo_source_revision(&source, &repo.rev, terminal_prompt).await?;
        git::checkout_repo_from_source(
            &source,
            repo.source(),
            &revision,
            temp.path(),
            terminal_prompt,
        )
        .await?;
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

    pub(crate) async fn lock_async(&self) -> Result<LockedFile, std::io::Error> {
        LockedFile::acquire(self.path.join(".lock"), "store").await
    }

    pub(crate) async fn repo_source_lock(
        &self,
        source: &str,
    ) -> Result<LockedFile, std::io::Error> {
        LockedFile::acquire(
            self.repo_source_locks_dir()
                .join(format!("{}.lock", Self::repo_source_key(source))),
            format!("repo source `{source}`"),
        )
        .await
    }

    /// Returns the path to where a remote repo would be stored.
    pub(crate) fn repo_path(&self, repo: &RemoteRepo) -> PathBuf {
        self.repos_dir().join(Self::repo_key(repo))
    }

    /// Returns the path to the shared bare source for a remote repo.
    pub(crate) fn repo_source_path(&self, source: &str) -> PathBuf {
        self.repo_sources_dir().join(Self::repo_source_key(source))
    }

    /// Returns the store key (directory name) for a remote repo.
    pub(crate) fn repo_key(repo: &RemoteRepo) -> String {
        let mut hasher = SeaHasher::new();
        repo.source().hash(&mut hasher);
        repo.rev.hash(&mut hasher);
        to_hex(hasher.finish())
    }

    /// Returns the store key for the shared bare source of a remote repo.
    pub(crate) fn repo_source_key(source: &str) -> String {
        let mut hasher = SeaHasher::new();
        source.hash(&mut hasher);
        to_hex(hasher.finish())
    }

    pub(crate) fn repo_sources_dir(&self) -> PathBuf {
        self.path.join("repo-sources")
    }

    fn repo_source_locks_dir(&self) -> PathBuf {
        self.repo_sources_dir().join(".locks")
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
        let mut changed = false;
        for config_path in config_paths {
            changed |= tracked.insert(config_path.to_path_buf());
        }

        if !changed {
            return Ok(());
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
    Dotnet,
    Deno,
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
