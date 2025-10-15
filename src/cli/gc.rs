use std::collections::HashSet;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use constants::{ALT_CONFIG_FILE, CONFIG_FILE};
use tracing::debug;

use crate::cli::ExitStatus;
use crate::config::{self, RemoteRepo};
use crate::git::GIT_ROOT;
use crate::printer::Printer;
use crate::store::Store;
use crate::workspace::Workspace;

/// Garbage collect unused repositories and cache data.
pub(crate) async fn gc(store: &Store, printer: Printer) -> Result<ExitStatus> {
    let _lock = store.lock_async().await?;

    let repos_removed = gc_repos(store).await?;

    writeln!(printer.stdout(), "{repos_removed} repo(s) removed.")?;

    Ok(ExitStatus::Success)
}

/// Remove unused repositories from the store.
async fn gc_repos(store: &Store) -> Result<usize> {
    // Get all cached config files by walking the git workspace
    let live_configs = find_live_configs(store)?;

    // Get all repositories currently in the store
    let all_repos = get_all_stored_repos(store)?;
    let mut unused_repos = all_repos.clone();

    // Mark repos as used based on live configurations
    for config_path in &live_configs {
        if let Err(e) = mark_used_repos_from_config(store, config_path, &mut unused_repos).await {
            debug!("Failed to process config {}: {}", config_path.display(), e);
            continue;
        }
    }

    // Remove unused repositories
    let removed_count = unused_repos.len();
    for (repo_key, repo_path) in unused_repos {
        debug!(
            "Removing unused repo: {} at {}",
            repo_key,
            repo_path.display()
        );
        if let Err(e) = fs_err::remove_dir_all(&repo_path) {
            debug!("Failed to remove repo at {}: {}", repo_path.display(), e);
        }
    }

    Ok(removed_count)
}

/// Find all live configuration files in the current workspace.
fn find_live_configs(store: &Store) -> Result<Vec<PathBuf>> {
    let mut configs = Vec::new();

    // Try to find workspace from current directory or git root
    let workspace_root = if let Ok(git_root) = GIT_ROOT.as_ref() {
        git_root.clone()
    } else {
        std::env::current_dir()?
    };

    // Load workspace to get all configuration files
    if let Ok(workspace) = Workspace::discover(store, workspace_root.clone(), None, None, false) {
        for project in workspace.projects() {
            if project.config_file().exists() {
                configs.push(project.config_file().to_path_buf());
            }
        }
    }

    // Also check for config files in common locations
    let common_config_names = [
        constants::CONFIG_FILE,     // .pre-commit-config.yaml
        constants::ALT_CONFIG_FILE, // .pre-commit-config.yml
    ];

    for config_name in &common_config_names {
        let config_path = workspace_root.join(config_name);
        if config_path.exists() && !configs.contains(&config_path) {
            configs.push(config_path);
        }
    }

    Ok(configs)
}
/// Get all repositories currently stored in the store.
fn get_all_stored_repos(store: &Store) -> Result<HashSet<(String, PathBuf)>> {
    let mut repos = HashSet::new();
    let repos_dir = store.repos_dir();

    if !repos_dir.exists() {
        return Ok(repos);
    }

    let mut entries = fs_err::read_dir(&repos_dir)?;
    while let Some(entry) = entries.next().transpose()? {
        let path = entry.path();
        if path.is_dir() {
            let metadata_file = path.join(".prek-repo.json");
            if metadata_file.exists() {
                if let Ok(content) = fs_err::read_to_string(&metadata_file) {
                    if let Ok(remote_repo) = serde_json::from_str::<RemoteRepo>(&content) {
                        let repo_key = format!("{}:{}", remote_repo.repo, remote_repo.rev);
                        repos.insert((repo_key, path));
                    }
                }
            }
        }
    }

    Ok(repos)
}

/// Mark repositories as used based on a configuration file.
async fn mark_used_repos_from_config(
    store: &Store,
    config_path: &Path,
    unused_repos: &mut HashSet<(String, PathBuf)>,
) -> Result<()> {
    let config = match config::read_config(config_path) {
        Ok(config) => config,
        Err(e) => {
            debug!("Failed to read config {}: {}", config_path.display(), e);
            return Ok(());
        }
    };

    for repo in &config.repos {
        if let config::Repo::Remote(remote_repo) = repo {
            mark_remote_repo_used(store, remote_repo, unused_repos).await?;
        }
    }

    Ok(())
}

/// Mark a remote repository as used.
async fn mark_remote_repo_used(
    _store: &Store,
    remote_repo: &RemoteRepo,
    unused_repos: &mut HashSet<(String, PathBuf)>,
) -> Result<()> {
    let repo_key = format!("{}:{}", remote_repo.repo, remote_repo.rev);

    // Remove from unused set - we don't need the path for removal
    unused_repos.retain(|(key, _)| key != &repo_key);

    Ok(())
}
