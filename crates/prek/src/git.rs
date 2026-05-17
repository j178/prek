use std::collections::HashSet;
use std::error::Error as StdError;
use std::io::Read as _;
use std::num::NonZeroU32;
use std::ops::ControlFlow;
use std::path::{Path, PathBuf};
use std::str::Utf8Error;
use std::sync::LazyLock;
use std::sync::atomic::AtomicBool;

use anyhow::Result;
use gix::bstr::ByteSlice as _;
use prek_consts::env_vars::EnvVars;
use rustc_hash::FxHashSet;
use similar::TextDiff;
use tracing::{debug, instrument, warn};

use crate::fs::{self, PathClean};
use crate::process;
#[cfg(test)]
use crate::process::Cmd;
use crate::process::StatusError;

#[derive(Debug, thiserror::Error)]
pub(crate) enum Error {
    #[error(transparent)]
    Command(#[from] process::Error),

    #[error("Git operation failed")]
    Gitoxide(#[source] Box<dyn StdError + Send + Sync>),

    #[error("Failed to find git: {0}")]
    GitNotFound(#[from] which::Error),

    #[error("Git repository has no working tree")]
    NoWorktree,

    #[error("Cannot write a tree from an index with unmerged paths")]
    UnmergedIndex,

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    UTF8(#[from] Utf8Error),

    #[error(
        "Git resolved hooks directory to the current directory (`{0}`). Unset `core.hooksPath` or set it to a real directory path."
    )]
    InvalidHooksPath(PathBuf),
}

impl Error {
    fn gitoxide(err: impl StdError + Send + Sync + 'static) -> Self {
        Self::Gitoxide(Box::new(err))
    }
}

pub(crate) static GIT: LazyLock<Result<PathBuf, which::Error>> =
    LazyLock::new(|| which::which("git"));

pub(crate) static GIT_ROOT: LazyLock<Result<PathBuf, Error>> = LazyLock::new(|| {
    get_root()
        .map(|root| dunce::canonicalize(&root).unwrap_or(root))
        .inspect(|root| {
            debug!("Git root: {}", root.display());
        })
});

/// Remove some `GIT_` environment variables exposed by `git`.
///
/// For some commands, like `git commit -a` or `git commit -p`, git creates a `.git/index.lock` file
/// and set `GIT_INDEX_FILE` to point to it.
/// We need to keep the `GIT_INDEX_FILE` env var to make sure `git write-tree` works correctly.
/// <https://stackoverflow.com/questions/65639403/git-pre-commit-hook-how-can-i-get-added-modified-files-when-commit-with-a-flag/65647202#65647202>
pub(crate) static GIT_ENV_TO_REMOVE: LazyLock<Vec<(String, String)>> = LazyLock::new(|| {
    let keep = &[
        "GIT_EXEC_PATH",
        "GIT_SSH",
        "GIT_SSH_COMMAND",
        "GIT_SSL_CAINFO",
        "GIT_SSL_NO_VERIFY",
        "GIT_CONFIG_COUNT",
        "GIT_CONFIG_PARAMETERS",
        "GIT_HTTP_PROXY_AUTHMETHOD",
        "GIT_ALLOW_PROTOCOL",
        "GIT_ASKPASS",
    ];

    std::env::vars()
        .filter(|(k, _)| {
            k.starts_with("GIT_")
                && !k.starts_with("GIT_CONFIG_KEY_")
                && !k.starts_with("GIT_CONFIG_VALUE_")
                && !keep.contains(&k.as_str())
        })
        .collect()
});

#[cfg(test)]
pub(crate) fn git_cmd(summary: &str) -> Result<Cmd, Error> {
    let mut cmd = Cmd::new(GIT.as_ref().map_err(|&e| Error::GitNotFound(e))?, summary);
    cmd.arg("-c").arg("core.useBuiltinFSMonitor=false");

    Ok(cmd)
}

fn discover_repo(path: &Path) -> Result<gix::Repository, Error> {
    gix::discover_with_environment_overrides(path).map_err(Error::gitoxide)
}

fn current_repo() -> Result<gix::Repository, Error> {
    discover_repo(Path::new("."))
}

fn open_index(repo: &gix::Repository) -> Result<gix::index::File, Error> {
    if let Some(index_path) = EnvVars::var_os(EnvVars::GIT_INDEX_FILE) {
        let mut index_path = PathBuf::from(index_path);
        if index_path.is_relative() && !index_path.exists() {
            let worktree_index_path = workdir(repo)?.join(&index_path);
            if worktree_index_path.exists() {
                index_path = worktree_index_path;
            }
        }

        if !index_path.exists() {
            return Ok(gix::index::File::from_state(
                gix::index::State::new(repo.object_hash()),
                index_path,
            ));
        }

        gix::index::File::at(
            index_path,
            repo.object_hash(),
            false,
            gix::index::decode::Options::default(),
        )
        .map_err(Error::gitoxide)
    } else {
        let index = repo.index_or_empty().map_err(Error::gitoxide)?;
        Ok(gix::index::File::clone(&index))
    }
}

fn absolute_path(path: &Path) -> Result<PathBuf, Error> {
    if path.is_absolute() {
        Ok(path.clean())
    } else {
        Ok(std::path::absolute(path)?.clean())
    }
}

fn workdir(repo: &gix::Repository) -> Result<&Path, Error> {
    repo.workdir().ok_or(Error::NoWorktree)
}

fn path_to_git_bytes(path: &Path) -> Vec<u8> {
    let path = gix::path::into_bstr(path);
    gix::path::to_unix_separators_on_windows(path.as_ref())
        .as_ref()
        .to_vec()
}

fn repo_relative_git_path(
    repo: &gix::Repository,
    cwd: &Path,
    path: &Path,
) -> Result<Option<Vec<u8>>, Error> {
    let cwd = absolute_path(cwd)?;
    let path = if path.is_absolute() {
        path.clean()
    } else {
        cwd.join(path).clean()
    };
    let repo_workdir = absolute_path(workdir(repo)?)?;
    let Ok(repo_relative) = path.strip_prefix(&repo_workdir) else {
        // A path outside the repository can never match an index entry.
        return Ok(Some(vec![0]));
    };
    let repo_relative = repo_relative.clean();
    if repo_relative.as_os_str().is_empty() || repo_relative == Path::new(".") {
        Ok(None)
    } else {
        Ok(Some(path_to_git_bytes(&repo_relative)))
    }
}

fn matches_git_prefix(path: &[u8], prefix: Option<&[u8]>) -> bool {
    let Some(prefix) = prefix else {
        return true;
    };
    path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|suffix| suffix.first() == Some(&b'/'))
}

fn path_relative_to_base(
    repo: &gix::Repository,
    path: &[u8],
    base: &Path,
) -> Result<PathBuf, Error> {
    let repo_relative = path_from_git_bytes(path)?;
    Ok(fs::relative_to(
        absolute_path(workdir(repo)?)?.join(repo_relative),
        absolute_path(base)?,
    )?)
}

fn zsplit(s: &[u8]) -> Result<Vec<PathBuf>, Utf8Error> {
    s.split(|&b| b == b'\0')
        .filter(|slice| !slice.is_empty())
        .map(path_from_git_bytes)
        .collect()
}

#[cfg(unix)]
#[expect(clippy::unnecessary_wraps)]
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf, Utf8Error> {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    Ok(PathBuf::from(OsStr::from_bytes(bytes)))
}

#[cfg(not(unix))]
fn path_from_git_bytes(bytes: &[u8]) -> Result<PathBuf, Utf8Error> {
    str::from_utf8(bytes).map(PathBuf::from)
}

pub(crate) fn intent_to_add_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(root)?;
    let index = open_index(&repo)?;
    let prefix = repo_relative_git_path(&repo, root, Path::new("."))?;
    index
        .entries()
        .iter()
        .filter(|entry| {
            entry
                .flags
                .contains(gix::index::entry::Flags::INTENT_TO_ADD)
        })
        .filter_map(|entry| {
            let path = entry.path(&index);
            matches_git_prefix(path.as_ref(), prefix.as_deref())
                .then(|| path_relative_to_base(&repo, path.as_ref(), workdir(&repo)?))
        })
        .collect()
}

pub(crate) fn clear_intent_to_add_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(root)?;
    let mut index = open_index(&repo)?;
    let prefix = repo_relative_git_path(&repo, root, Path::new("."))?;
    let paths = index
        .entries()
        .iter()
        .filter(|entry| {
            entry
                .flags
                .contains(gix::index::entry::Flags::INTENT_TO_ADD)
        })
        .filter_map(|entry| {
            let path = entry.path(&index);
            let path: &[u8] = path.as_ref();
            matches_git_prefix(path, prefix.as_deref()).then(|| path.to_vec())
        })
        .collect::<Vec<_>>();

    if paths.is_empty() {
        return Ok(vec![]);
    }

    let files = paths
        .iter()
        .map(|path| path_relative_to_base(&repo, path, root))
        .collect::<Result<Vec<_>, _>>()?;
    let paths = paths.into_iter().collect::<HashSet<_>>();
    index.remove_entries(|_, path, _| {
        let path: &[u8] = path.as_ref();
        paths.contains::<[u8]>(path)
    });
    index.remove_tree();
    index
        .write(gix::index::write::Options::default())
        .map_err(Error::gitoxide)?;

    Ok(files)
}

pub(crate) fn restore_intent_to_add_files(root: &Path, files: &[PathBuf]) -> Result<(), Error> {
    if files.is_empty() {
        return Ok(());
    }

    let repo = discover_repo(root)?;
    let mut index = open_index(&repo)?;
    let empty_blob = repo.write_blob([]).map_err(Error::gitoxide)?.detach();

    for file in files {
        let Some(path) = repo_relative_git_path(&repo, root, file)? else {
            continue;
        };
        let path = path.as_bstr();
        let flags = gix::index::entry::Flags::EXTENDED | gix::index::entry::Flags::INTENT_TO_ADD;

        if let Some(entry) =
            index.entry_mut_by_path_and_stage(path, gix::index::entry::Stage::Unconflicted)
        {
            entry.stat = gix::index::entry::Stat::default();
            entry.id = empty_blob;
            entry.flags = flags;
            entry.mode = gix::index::entry::Mode::FILE;
        } else {
            index.dangerously_push_entry(
                gix::index::entry::Stat::default(),
                empty_blob,
                flags,
                gix::index::entry::Mode::FILE,
                path,
            );
        }
    }

    index.sort_entries();
    index.remove_tree();
    index
        .write(gix::index::write::Options::default())
        .map_err(Error::gitoxide)?;

    Ok(())
}

pub(crate) fn get_added_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    staged_paths(root, StagedPathFilter::Added, Some(root))
}

pub(crate) fn get_changed_files(old: &str, new: &str, root: &Path) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(root)?;
    if let Ok(paths) = changed_paths_from_merge_base(&repo, old, new, root) {
        return Ok(paths);
    }

    changed_paths_between_trees(
        &repo,
        rev_to_tree_id(&repo, old)?,
        rev_to_tree_id(&repo, new)?,
        root,
    )
}

#[instrument(level = "trace")]
pub(crate) fn ls_files(cwd: &Path, path: &Path) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(cwd)?;
    let index = open_index(&repo)?;
    let prefix = repo_relative_git_path(&repo, cwd, path)?;

    index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() == 0)
        .filter_map(|entry| {
            let path = entry.path(&index);
            matches_git_prefix(path.as_ref(), prefix.as_deref())
                .then(|| path_relative_to_base(&repo, path.as_ref(), cwd))
        })
        .collect()
}

pub(crate) fn get_git_dir() -> Result<PathBuf, Error> {
    Ok(current_repo()?.git_dir().to_path_buf())
}

pub(crate) fn get_git_common_dir() -> Result<PathBuf, Error> {
    Ok(current_repo()?.common_dir().to_path_buf())
}

pub(crate) fn get_git_hooks_dir() -> Result<PathBuf, Error> {
    // Ask Git for the effective hooks directory instead of reconstructing it
    // ourselves. That lets Git apply the full precedence chain for
    // `core.hooksPath`, including local/worktree config, linked worktrees, bare
    // + worktree layouts, and repo-owned config loaded through `include.path`
    // / `includeIf`.
    let repo = current_repo()?;
    let hooks_dir = repo
        .config_snapshot()
        .string("core.hooksPath")
        .map(|path| {
            repo.workdir()
                .unwrap_or_else(|| repo.common_dir())
                .join(gix::path::from_bstr(path))
        })
        .unwrap_or_else(|| repo.common_dir().join("hooks"));

    let cleaned = hooks_dir.clean();
    // `core.hooksPath=` is a particularly dangerous case: Git treats it as
    // configured, but resolves `--git-path hooks` to the current directory. If
    // we accepted that value, install/uninstall would write or remove hook
    // shims from the worktree root. Keep the explicit `core.hooksPath=.` case
    // working, but reject the empty-string variant.
    if cleaned == Path::new(".") && config_value_is_empty(None, "core.hooksPath")? {
        Err(Error::InvalidHooksPath(cleaned))
    } else {
        Ok(cleaned)
    }
}

#[derive(Clone, Copy)]
enum StagedPathFilter {
    Added,
    NotDeleted,
}

#[derive(Debug)]
pub(crate) struct StagedChangedEntry {
    pub(crate) head_mode: u32,
    pub(crate) index_mode: u32,
    pub(crate) head_hash: String,
    pub(crate) index_hash: String,
    pub(crate) path: PathBuf,
}

fn staged_paths(
    cwd: &Path,
    filter: StagedPathFilter,
    restrict_to: Option<&Path>,
) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(cwd)?;
    let index = open_index(&repo)?;
    let head_tree = repo.head_tree_id_or_empty().map_err(Error::gitoxide)?;
    let prefix = restrict_to
        .map(|path| repo_relative_git_path(&repo, path, Path::new(".")))
        .transpose()?
        .flatten();
    let output_base = match restrict_to {
        Some(path) => path,
        None => workdir(&repo)?,
    };
    let mut paths = Vec::new();

    repo.tree_index_status(
        head_tree.as_ref(),
        &index,
        None,
        gix::status::tree_index::TrackRenames::Disabled,
        |change, _, _| {
            let path = match change {
                gix::diff::index::ChangeRef::Addition { location, .. } => Some(location),
                gix::diff::index::ChangeRef::Modification { location, .. }
                | gix::diff::index::ChangeRef::Rewrite { location, .. }
                    if matches!(filter, StagedPathFilter::NotDeleted) =>
                {
                    Some(location)
                }
                _ => None,
            };

            if let Some(path) = path
                && matches_git_prefix(path.as_ref(), prefix.as_deref())
            {
                paths.push(path.as_ref().to_vec());
            }

            Ok::<_, std::convert::Infallible>(ControlFlow::Continue(()))
        },
    )
    .map_err(Error::gitoxide)?;

    paths
        .into_iter()
        .map(|path| path_relative_to_base(&repo, &path, output_base))
        .collect()
}

pub(crate) fn staged_changed_entries(work_dir: &Path) -> Result<Vec<StagedChangedEntry>, Error> {
    let repo = discover_repo(work_dir)?;
    let index = open_index(&repo)?;
    let head_tree = repo.head_tree_id_or_empty().map_err(Error::gitoxide)?;
    let prefix = repo_relative_git_path(&repo, work_dir, Path::new("."))?;
    let mut entries = Vec::new();

    repo.tree_index_status(
        head_tree.as_ref(),
        &index,
        None,
        gix::status::tree_index::TrackRenames::Disabled,
        |change, _, _| {
            if let gix::diff::index::ChangeRef::Modification {
                location,
                previous_entry_mode,
                previous_id,
                entry_mode,
                id,
                ..
            } = change
                && matches_git_prefix(location.as_ref(), prefix.as_deref())
            {
                entries.push(StagedChangedEntry {
                    head_mode: previous_entry_mode.bits(),
                    index_mode: entry_mode.bits(),
                    head_hash: previous_id.as_ref().to_hex().to_string(),
                    index_hash: id.as_ref().to_hex().to_string(),
                    path: path_from_git_bytes(location.as_ref())?,
                });
            }

            Ok::<_, Error>(ControlFlow::Continue(()))
        },
    )
    .map_err(Error::gitoxide)?;

    Ok(entries)
}

fn matches_path_filter(path: &Path, paths: &[&Path]) -> bool {
    paths.is_empty() || paths.contains(&path)
}

pub(crate) fn added_submodules_in_index(
    work_dir: &Path,
    paths: &[&Path],
) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(work_dir)?;
    let index = open_index(&repo)?;
    let head_tree = repo.head_tree_id_or_empty().map_err(Error::gitoxide)?;
    let prefix = repo_relative_git_path(&repo, work_dir, Path::new("."))?;
    let mut submodules = Vec::new();

    repo.tree_index_status(
        head_tree.as_ref(),
        &index,
        None,
        gix::status::tree_index::TrackRenames::Disabled,
        |change, _, _| {
            if let gix::diff::index::ChangeRef::Addition {
                location,
                entry_mode,
                ..
            } = change
                && entry_mode == gix::index::entry::Mode::COMMIT
                && matches_git_prefix(location.as_ref(), prefix.as_deref())
            {
                let path = path_relative_to_base(&repo, location.as_ref(), work_dir)?;
                if matches_path_filter(&path, paths) {
                    submodules.push(path);
                }
            }

            Ok::<_, Error>(ControlFlow::Continue(()))
        },
    )
    .map_err(Error::gitoxide)?;

    Ok(submodules)
}

fn rev_to_commit_id(repo: &gix::Repository, rev: &str) -> Result<gix::ObjectId, Error> {
    let spec = format!("{rev}^{{commit}}");
    Ok(repo
        .rev_parse_single(spec.as_str())
        .map_err(Error::gitoxide)?
        .detach())
}

fn rev_to_tree_id(repo: &gix::Repository, rev: &str) -> Result<gix::ObjectId, Error> {
    let spec = format!("{rev}^{{tree}}");
    Ok(repo
        .rev_parse_single(spec.as_str())
        .map_err(Error::gitoxide)?
        .detach())
}

fn commit_tree_id(repo: &gix::Repository, commit: gix::ObjectId) -> Result<gix::ObjectId, Error> {
    Ok(repo
        .find_commit(commit)
        .map_err(Error::gitoxide)?
        .tree_id()
        .map_err(Error::gitoxide)?
        .detach())
}

fn changed_paths_between_trees(
    repo: &gix::Repository,
    old_tree: gix::ObjectId,
    new_tree: gix::ObjectId,
    root: &Path,
) -> Result<Vec<PathBuf>, Error> {
    let old_tree = repo.find_tree(old_tree).map_err(Error::gitoxide)?;
    let new_tree = repo.find_tree(new_tree).map_err(Error::gitoxide)?;
    let prefix = repo_relative_git_path(repo, root, Path::new("."))?;
    let mut paths = Vec::new();

    let mut changes = old_tree.changes().map_err(Error::gitoxide)?;
    changes
        .for_each_to_obtain_tree(&new_tree, |change| {
            let path = match change {
                gix::object::tree::diff::Change::Addition { location, .. }
                | gix::object::tree::diff::Change::Modification { location, .. }
                | gix::object::tree::diff::Change::Rewrite { location, .. } => Some(location),
                gix::object::tree::diff::Change::Deletion { .. } => None,
            };

            if let Some(path) = path
                && matches_git_prefix(path.as_ref(), prefix.as_deref())
            {
                paths.push(path_relative_to_base(repo, path.as_ref(), workdir(repo)?)?);
            }

            Ok::<_, Error>(ControlFlow::Continue(()))
        })
        .map_err(Error::gitoxide)?;

    Ok(paths)
}

fn changed_paths_from_merge_base(
    repo: &gix::Repository,
    old: &str,
    new: &str,
    root: &Path,
) -> Result<Vec<PathBuf>, Error> {
    let old = rev_to_commit_id(repo, old)?;
    let new = rev_to_commit_id(repo, new)?;
    let merge_base = repo.merge_base(old, new).map_err(Error::gitoxide)?.detach();
    changed_paths_between_trees(
        repo,
        commit_tree_id(repo, merge_base)?,
        commit_tree_id(repo, new)?,
        root,
    )
}

pub(crate) fn added_submodules_between_refs(
    work_dir: &Path,
    from_ref: &str,
    to_ref: &str,
    paths: &[&Path],
) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(work_dir)?;
    let from = rev_to_commit_id(&repo, from_ref)?;
    let to = rev_to_commit_id(&repo, to_ref)?;
    let merge_base = repo.merge_base(from, to).map_err(Error::gitoxide)?.detach();
    let merge_base_tree = repo
        .find_commit(merge_base)
        .map_err(Error::gitoxide)?
        .tree_id()
        .map_err(Error::gitoxide)?
        .detach();
    let to_tree = repo
        .find_commit(to)
        .map_err(Error::gitoxide)?
        .tree_id()
        .map_err(Error::gitoxide)?
        .detach();
    let merge_base_tree = repo.find_tree(merge_base_tree).map_err(Error::gitoxide)?;
    let to_tree = repo.find_tree(to_tree).map_err(Error::gitoxide)?;
    let prefix = repo_relative_git_path(&repo, work_dir, Path::new("."))?;
    let mut submodules = Vec::new();

    let mut changes = merge_base_tree.changes().map_err(Error::gitoxide)?;
    changes.options(|opts| {
        opts.track_rewrites(None);
    });
    changes
        .for_each_to_obtain_tree(&to_tree, |change| {
            if let gix::object::tree::diff::Change::Addition {
                location,
                entry_mode,
                ..
            } = change
                && entry_mode.value() == 0o160_000
                && matches_git_prefix(location.as_ref(), prefix.as_deref())
            {
                let path = path_relative_to_base(&repo, location.as_ref(), work_dir)?;
                if matches_path_filter(&path, paths) {
                    submodules.push(path);
                }
            }

            Ok::<_, Error>(ControlFlow::Continue(()))
        })
        .map_err(Error::gitoxide)?;

    Ok(submodules)
}

pub(crate) fn get_staged_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    staged_paths(root, StagedPathFilter::NotDeleted, None)
}

pub(crate) fn files_not_staged(files: &[&Path]) -> Result<Vec<PathBuf>> {
    let repo = current_repo()?;
    let index = open_index(&repo)?;
    let repo_workdir = absolute_path(workdir(&repo)?)?;
    let tracks_file_mode = repo_tracks_file_mode(&repo);
    let mut filter_pipeline = filter_pipeline(&repo, &index)?;
    let path_filters = files
        .iter()
        .map(|path| {
            let path = absolute_path(path)?;
            let Ok(repo_relative) = path.strip_prefix(&repo_workdir) else {
                return Ok(vec![0]);
            };
            Ok(path_to_git_bytes(&repo_relative.clean()))
        })
        .collect::<Result<Vec<_>, Error>>()?;
    let mut paths = Vec::new();

    for entry in index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() == 0)
    {
        let path = entry.path(&index);
        let path_bytes: &[u8] = path.as_ref();
        if !path_filters.is_empty()
            && !path_filters
                .iter()
                .any(|filter| path_bytes == filter.as_slice())
        {
            continue;
        }
        if !index_entry_worktree_changed(
            &repo,
            &index,
            &mut filter_pipeline,
            entry,
            &repo_workdir,
            tracks_file_mode,
        )? {
            continue;
        }

        paths.push(path_relative_to_base(&repo, path_bytes, &repo_workdir)?);
    }

    Ok(paths)
}

pub(crate) fn files_not_staged_under(root: &Path) -> Result<Vec<PathBuf>> {
    let repo = discover_repo(root)?;
    let index = open_index(&repo)?;
    let repo_workdir = absolute_path(workdir(&repo)?)?;
    let tracks_file_mode = repo_tracks_file_mode(&repo);
    let mut filter_pipeline = filter_pipeline(&repo, &index)?;
    let prefix = repo_relative_git_path(&repo, root, Path::new("."))?;
    let mut paths = Vec::new();

    for entry in index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() == 0)
    {
        let path = entry.path(&index);
        let path_bytes: &[u8] = path.as_ref();
        if !matches_git_prefix(path_bytes, prefix.as_deref()) {
            continue;
        }
        if !index_entry_worktree_changed(
            &repo,
            &index,
            &mut filter_pipeline,
            entry,
            &repo_workdir,
            tracks_file_mode,
        )? {
            continue;
        }

        paths.push(path_relative_to_base(&repo, path_bytes, root)?);
    }

    Ok(paths)
}

pub(crate) fn has_unmerged_paths() -> Result<bool, Error> {
    let repo = current_repo()?;
    Ok(open_index(&repo)?
        .entries()
        .iter()
        .any(|entry| entry.stage_raw() != 0))
}

pub(crate) fn has_diff(rev: &str, path: &Path) -> Result<bool> {
    let repo = discover_repo(path)?;
    let index = open_index(&repo)?;
    let rev_tree = rev_to_tree_id(&repo, rev)?;
    let mut index_differs = false;
    repo.tree_index_status(
        rev_tree.as_ref(),
        &index,
        None,
        gix::status::tree_index::TrackRenames::Disabled,
        |_, _, _| {
            index_differs = true;
            Ok::<_, std::convert::Infallible>(ControlFlow::Break(()))
        },
    )
    .map_err(Error::gitoxide)?;
    if index_differs {
        return Ok(true);
    }

    for item in repo
        .status(gix::progress::Discard)
        .map_err(Error::gitoxide)?
        .index(index.into())
        .index_worktree_options_mut(|opts| {
            opts.dirwalk_options = None;
        })
        .into_index_worktree_iter(Vec::new())
        .map_err(Error::gitoxide)?
    {
        if item.map_err(Error::gitoxide)?.summary().is_some() {
            return Ok(true);
        }
    }

    Ok(false)
}

pub(crate) fn is_in_merge_conflict() -> Result<bool, Error> {
    let git_dir = get_git_dir()?;
    Ok(git_dir.join("MERGE_HEAD").try_exists()? && git_dir.join("MERGE_MSG").try_exists()?)
}

pub(crate) async fn get_conflicted_files(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(root)?;
    let index = open_index(&repo)?;
    let prefix = repo_relative_git_path(&repo, root, Path::new("."))?;
    let mut conflicts = index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() != 0)
        .filter_map(|entry| {
            let path = entry.path(&index);
            matches_git_prefix(path.as_ref(), prefix.as_deref())
                .then(|| path_relative_to_base(&repo, path.as_ref(), workdir(&repo)?))
        })
        .collect::<Result<HashSet<PathBuf>, Error>>()?;

    conflicts.extend(parse_merge_msg_for_conflicts().await?);

    Ok(conflicts.into_iter().collect())
}

async fn parse_merge_msg_for_conflicts() -> Result<Vec<PathBuf>, Error> {
    let git_dir = get_git_dir()?;
    let merge_msg = git_dir.join("MERGE_MSG");
    let content = fs_err::tokio::read_to_string(&merge_msg).await?;
    let conflicts = content
        .lines()
        // Conflicted files start with tabs
        .filter(|line| line.starts_with('\t') || line.starts_with("#\t"))
        .map(|line| line.trim_start_matches('#').trim().to_string())
        .map(PathBuf::from)
        .collect();

    Ok(conflicts)
}

#[instrument(level = "trace")]
pub(crate) fn worktree_change_signature(path: &Path) -> Result<Vec<u8>, Error> {
    let changes = worktree_changes(path)?;
    let mut signature = Vec::new();

    for change in changes.entries {
        signature.extend_from_slice(&change.path);
        signature.push(0);
        signature.extend_from_slice(format!("{:?}\0", change.mode).as_bytes());
        signature.extend_from_slice(change.index_id.as_bytes());
        signature.push(0);

        if let Some(content) = change.worktree_content {
            signature.extend_from_slice(&content);
        } else {
            signature.extend_from_slice(b"<missing>");
        }
        signature.push(0);
    }

    Ok(signature)
}

struct WorktreeChanges {
    object_hash: gix::hash::Kind,
    entries: Vec<WorktreeChange>,
}

struct WorktreeChange {
    path: Vec<u8>,
    mode: gix::index::entry::Mode,
    index_id: gix::ObjectId,
    index_content: Vec<u8>,
    worktree_content: Option<Vec<u8>>,
}

fn worktree_changes(path: &Path) -> Result<WorktreeChanges, Error> {
    let repo = discover_repo(path)?;
    let index = open_index(&repo)?;
    let prefix = repo_relative_git_path(&repo, path, Path::new("."))?;
    let repo_workdir = absolute_path(workdir(&repo)?)?;
    let tracks_file_mode = repo_tracks_file_mode(&repo);
    let mut filter_pipeline = filter_pipeline(&repo, &index)?;
    let mut entries = Vec::new();

    for entry in index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() == 0)
    {
        let git_path = entry.path(&index);
        let git_path_bytes: &[u8] = git_path.as_ref();
        if !matches_git_prefix(git_path_bytes, prefix.as_deref()) {
            continue;
        }
        if !index_entry_worktree_changed(
            &repo,
            &index,
            &mut filter_pipeline,
            entry,
            &repo_workdir,
            tracks_file_mode,
        )? {
            continue;
        }

        let repo_relative_path = path_from_git_bytes(git_path_bytes)?;
        let worktree_path = repo_workdir.join(&repo_relative_path);
        let worktree_content = worktree_content_as_git(
            &index,
            &mut filter_pipeline,
            &repo_relative_path,
            &worktree_path,
        )?;
        entries.push(WorktreeChange {
            path: git_path_bytes.to_vec(),
            mode: entry.mode,
            index_id: entry.id,
            index_content: index_content(&repo, entry)?,
            worktree_content,
        });
    }

    Ok(WorktreeChanges {
        object_hash: repo.object_hash(),
        entries,
    })
}

#[cfg(unix)]
fn metadata_is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn metadata_is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn repo_tracks_file_mode(repo: &gix::Repository) -> bool {
    repo.config_snapshot()
        .boolean("core.fileMode")
        .unwrap_or(true)
}

fn index_content(repo: &gix::Repository, entry: &gix::index::Entry) -> Result<Vec<u8>, Error> {
    Ok(repo
        .find_object(entry.id)
        .map_err(Error::gitoxide)?
        .data
        .clone())
}

fn filter_pipeline<'repo>(
    repo: &'repo gix::Repository,
    index: &gix::index::File,
) -> Result<gix::filter::Pipeline<'repo>, Error> {
    let attributes = repo
        .attributes_only(
            index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        )
        .map_err(Error::gitoxide)?;
    gix::filter::Pipeline::new(repo, attributes.detach()).map_err(Error::gitoxide)
}

fn worktree_content(path: &Path) -> Result<Option<Vec<u8>>, Error> {
    match fs_err::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Ok(Some(path_to_git_bytes(&fs_err::read_link(path)?)))
        }
        Ok(metadata) if metadata.is_file() => Ok(Some(fs_err::read(path)?)),
        Ok(_) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn worktree_content_as_git(
    index: &gix::index::File,
    filter_pipeline: &mut gix::filter::Pipeline<'_>,
    repo_relative_path: &Path,
    worktree_path: &Path,
) -> Result<Option<Vec<u8>>, Error> {
    let metadata = match fs_err::symlink_metadata(worktree_path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    if metadata.file_type().is_symlink() {
        return Ok(Some(path_to_git_bytes(&fs_err::read_link(worktree_path)?)));
    }
    if !metadata.is_file() {
        return Ok(None);
    }

    let file = fs_err::File::open(worktree_path)?;
    let mut content = Vec::new();
    match filter_pipeline
        .convert_to_git(file, repo_relative_path, index)
        .map_err(Error::gitoxide)?
    {
        gix::filter::plumbing::pipeline::convert::ToGitOutcome::Unchanged(mut file) => {
            file.read_to_end(&mut content)?;
        }
        gix::filter::plumbing::pipeline::convert::ToGitOutcome::Buffer(buf) => {
            content.extend_from_slice(buf);
        }
        gix::filter::plumbing::pipeline::convert::ToGitOutcome::Process(mut read) => {
            read.read_to_end(&mut content)?;
        }
    }

    Ok(Some(content))
}

fn worktree_mode(
    path: &Path,
    index_mode: gix::index::entry::Mode,
    tracks_file_mode: bool,
) -> Result<Option<gix::index::entry::Mode>, Error> {
    match fs_err::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Ok(Some(gix::index::entry::Mode::SYMLINK))
        }
        Ok(metadata) if metadata.is_file() => {
            let executable = if tracks_file_mode {
                metadata_is_executable(&metadata)
            } else {
                index_mode == gix::index::entry::Mode::FILE_EXECUTABLE
            };
            let mode = if executable {
                gix::index::entry::Mode::FILE_EXECUTABLE
            } else {
                gix::index::entry::Mode::FILE
            };
            Ok(Some(mode))
        }
        Ok(_) => Ok(None),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn index_entry_worktree_changed(
    repo: &gix::Repository,
    index: &gix::index::File,
    filter_pipeline: &mut gix::filter::Pipeline<'_>,
    entry: &gix::index::Entry,
    repo_workdir: &Path,
    tracks_file_mode: bool,
) -> Result<bool, Error> {
    if matches!(
        entry.mode,
        gix::index::entry::Mode::COMMIT | gix::index::entry::Mode::DIR
    ) {
        return Ok(false);
    }

    let path = repo_workdir.join(path_from_git_bytes(entry.path(index).as_ref())?);
    if worktree_mode(&path, entry.mode, tracks_file_mode)? != Some(entry.mode) {
        return Ok(true);
    }

    let index_content = index_content(repo, entry)?;
    if worktree_content(&path)?.as_deref() == Some(index_content.as_slice()) {
        return Ok(false);
    }

    let repo_relative_path = path_from_git_bytes(entry.path(index).as_ref())?;
    Ok(
        worktree_content_as_git(index, filter_pipeline, &repo_relative_path, &path)?.as_deref()
            != Some(index_content.as_slice()),
    )
}

fn colorize_diff(diff: &str) -> String {
    let mut colored = String::with_capacity(diff.len());
    for line in diff.split_inclusive('\n') {
        let color = if line.starts_with('+') && !line.starts_with("+++") {
            Some("32")
        } else if line.starts_with('-') && !line.starts_with("---") {
            Some("31")
        } else if line.starts_with("@@") {
            Some("36")
        } else {
            None
        };

        if let Some(color) = color {
            colored.push_str("\x1b[");
            colored.push_str(color);
            colored.push('m');
            colored.push_str(line);
            colored.push_str("\x1b[0m");
        } else {
            colored.push_str(line);
        }
    }
    colored
}

fn is_binary_content(content: &[u8]) -> bool {
    content.contains(&0) || std::str::from_utf8(content).is_err()
}

pub(crate) fn worktree_diff(path: &Path, color: bool) -> Result<String, Error> {
    let changes = worktree_changes(path)?;
    let mut output = String::new();

    for change in changes.entries {
        let old = change.index_content;
        let new = change.worktree_content.unwrap_or_default();
        let new_id = gix::objs::compute_hash(changes.object_hash, gix::objs::Kind::Blob, &new)
            .map_err(Error::gitoxide)?;

        let path = change.path.as_bstr().to_str_lossy();
        output.push_str("diff --git a/");
        output.push_str(&path);
        output.push_str(" b/");
        output.push_str(&path);
        output.push('\n');
        output.push_str("index ");
        output.push_str(&change.index_id.to_hex_with_len(7).to_string());
        output.push_str("..");
        output.push_str(&new_id.to_hex_with_len(7).to_string());
        output.push(' ');
        output.push_str(match change.mode {
            gix::index::entry::Mode::FILE_EXECUTABLE => "100755",
            gix::index::entry::Mode::SYMLINK => "120000",
            _ => "100644",
        });
        output.push('\n');

        if is_binary_content(&old) || is_binary_content(&new) {
            output.push_str("Binary files a/");
            output.push_str(&path);
            output.push_str(" and b/");
            output.push_str(&path);
            output.push_str(" differ\n");
            continue;
        }

        let old = std::str::from_utf8(&old)?;
        let new = std::str::from_utf8(&new)?;
        let diff = TextDiff::from_lines(old, new)
            .unified_diff()
            .context_radius(3)
            .header(&format!("a/{path}"), &format!("b/{path}"))
            .to_string();
        if color {
            output.push_str(&colorize_diff(&diff));
        } else {
            output.push_str(&diff);
        }
    }

    Ok(output)
}

fn remove_worktree_path(path: &Path) -> Result<(), Error> {
    match fs_err::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs_err::remove_dir_all(path)?;
        }
        Ok(_) => {
            fs_err::remove_file(path)?;
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

fn checkout_index_entry(
    repo: &gix::Repository,
    index: &gix::index::File,
    entry: &gix::index::Entry,
    path: &[u8],
) -> Result<(), Error> {
    let worktree_path = workdir(repo)?.join(path_from_git_bytes(path)?);
    if let Some(parent) = worktree_path.parent() {
        fs_err::create_dir_all(parent)?;
    }

    match entry.mode {
        gix::index::entry::Mode::FILE | gix::index::entry::Mode::FILE_EXECUTABLE => {
            remove_worktree_path(&worktree_path)?;
            let content = repo
                .find_object(entry.id)
                .map_err(Error::gitoxide)?
                .data
                .clone();
            fs_err::write(&worktree_path, content)?;

            #[cfg(unix)]
            {
                use std::fs::Permissions;
                use std::os::unix::fs::PermissionsExt as _;

                let mode = if entry.mode == gix::index::entry::Mode::FILE_EXECUTABLE {
                    0o755
                } else {
                    0o644
                };
                fs_err::set_permissions(&worktree_path, Permissions::from_mode(mode))?;
            }
        }
        gix::index::entry::Mode::SYMLINK => {
            let target = repo
                .find_object(entry.id)
                .map_err(Error::gitoxide)?
                .data
                .clone();
            remove_worktree_path(&worktree_path)?;
            #[cfg(unix)]
            {
                use std::ffi::OsStr;
                use std::os::unix::ffi::OsStrExt as _;
                std::os::unix::fs::symlink(OsStr::from_bytes(&target), &worktree_path)?;
            }
            #[cfg(not(unix))]
            {
                fs_err::write(&worktree_path, target)?;
            }
        }
        gix::index::entry::Mode::COMMIT | gix::index::entry::Mode::DIR => {}
        _ => {
            return Err(Error::gitoxide(std::io::Error::other(
                "unknown index entry mode",
            )));
        }
    }

    let _ = index;
    Ok(())
}

pub(crate) fn restore_index_paths(root: &Path, files: &[PathBuf]) -> Result<(), Error> {
    if files.is_empty() {
        return Ok(());
    }

    let repo = discover_repo(root)?;
    let index = open_index(&repo)?;
    for file in files {
        let Some(path) = repo_relative_git_path(&repo, root, file)? else {
            continue;
        };
        let path = path.as_bstr();
        if let Some(entry) =
            index.entry_by_path_and_stage(path, gix::index::entry::Stage::Unconflicted)
        {
            checkout_index_entry(&repo, &index, entry, path.as_ref())?;
        } else {
            remove_worktree_path(&workdir(&repo)?.join(path_from_git_bytes(path.as_ref())?))?;
        }
    }

    Ok(())
}

/// Create a tree object from the current index.
///
/// The name of the new tree object is printed to standard output.
/// The index must be in a fully merged state.
pub(crate) fn write_tree() -> Result<String, Error> {
    let repo = current_repo()?;
    let index = open_index(&repo)?;
    Ok(write_index_tree(&repo, &index)?.to_string())
}

pub(crate) fn write_index_tree(
    repo: &gix::Repository,
    index: &gix::index::File,
) -> Result<gix::ObjectId, Error> {
    let mut editor = repo.empty_tree().edit().map_err(Error::gitoxide)?;

    for entry in index.entries() {
        if entry.stage_raw() != 0 {
            return Err(Error::UnmergedIndex);
        }

        let kind = match entry.mode {
            gix::index::entry::Mode::FILE => gix::objs::tree::EntryKind::Blob,
            gix::index::entry::Mode::FILE_EXECUTABLE => gix::objs::tree::EntryKind::BlobExecutable,
            gix::index::entry::Mode::SYMLINK => gix::objs::tree::EntryKind::Link,
            gix::index::entry::Mode::COMMIT => gix::objs::tree::EntryKind::Commit,
            gix::index::entry::Mode::DIR => gix::objs::tree::EntryKind::Tree,
            _ => {
                return Err(Error::gitoxide(std::io::Error::other(
                    "unknown index entry mode",
                )));
            }
        };
        editor
            .upsert(entry.path(index), kind, entry.id)
            .map_err(Error::gitoxide)?;
    }

    Ok(editor.write().map_err(Error::gitoxide)?.detach())
}

/// Get the path of the top-level directory of the working tree.
#[instrument(level = "trace")]
pub(crate) fn get_root() -> Result<PathBuf, Error> {
    Ok(current_repo()?
        .workdir()
        .ok_or(Error::NoWorktree)?
        .to_path_buf())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TerminalPrompt {
    Disabled,
    Enabled,
}

impl TerminalPrompt {
    fn credential_prompt_config_value(self) -> &'static str {
        match self {
            Self::Disabled => "false",
            Self::Enabled => "true",
        }
    }
}

/// Return whether a git clone failure looks like an authentication error.
pub(crate) fn is_auth_error(err: &Error) -> bool {
    let error = match err {
        Error::Command(process::Error::Status {
            error:
                StatusError {
                    output: Some(output),
                    ..
                },
            ..
        }) => String::from_utf8_lossy(&output.stderr).to_lowercase(),
        Error::Gitoxide(err) => format!("{err:?}").to_lowercase(),
        _ => return false,
    };

    [
        "terminal prompts disabled",
        "terminal prompts are disabled",
        "could not read username",
        "could not read password",
        "authentication failed",
        "http basic: access denied",
        "missing or invalid credentials",
        "could not authenticate to server",
    ]
    .iter()
    .any(|needle| error.contains(needle))
}

fn looks_like_object_id(rev: &str) -> bool {
    (7..=64).contains(&rev.len()) && rev.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn default_remote_url(repo: &gix::Repository) -> Result<Option<gix::Url>, Error> {
    repo.find_default_remote(gix::remote::Direction::Fetch)
        .map(|remote| {
            remote
                .map_err(Error::gitoxide)
                .map(|remote| remote.url(gix::remote::Direction::Fetch).cloned())
        })
        .transpose()
        .map(Option::flatten)
}

fn join_url_path(base: &[u8], relative: &[u8]) -> Vec<u8> {
    let absolute = base.first() == Some(&b'/');
    let mut components = base
        .split(|byte| *byte == b'/')
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    components.pop();

    for component in relative.split(|byte| *byte == b'/') {
        match component {
            b"" | b"." => {}
            b".." => {
                components.pop();
            }
            _ => components.push(component),
        }
    }

    let mut path = Vec::new();
    if absolute {
        path.push(b'/');
    }
    for (index, component) in components.iter().enumerate() {
        if index > 0 {
            path.push(b'/');
        }
        path.extend_from_slice(component);
    }
    path
}

fn resolve_submodule_url(repo: &gix::Repository, mut url: gix::Url) -> Result<gix::Url, Error> {
    if url.scheme.as_str() != "file" {
        return Ok(url);
    }

    let path = path_from_git_bytes(url.path.as_ref())?;
    if path.is_absolute() {
        return Ok(url);
    }

    if let Some(mut parent_url) = default_remote_url(repo)? {
        parent_url.path = join_url_path(parent_url.path.as_ref(), url.path.as_ref()).into();
        return Ok(parent_url);
    }

    url.path = path_to_git_bytes(&workdir(repo)?.join(path).clean()).into();
    Ok(url)
}

fn remove_empty_directory(path: &Path) -> Result<(), Error> {
    match fs_err::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            if fs_err::read_dir(path)?.next().transpose()?.is_none() {
                fs_err::remove_dir(path)?;
            }
        }
        Ok(_) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

fn checkout_revision(repo: &gix::Repository, rev: &str) -> Result<(), Error> {
    let commit = rev_to_commit_id(repo, rev)?;
    let tree = commit_tree_id(repo, commit)?;
    let index = gix::index::State::from_tree(
        tree.as_ref(),
        &repo.objects,
        gix::index::validate::path::component::Options::default(),
    )
    .map_err(Error::gitoxide)?;
    let mut index = gix::index::File::from_state(index, repo.index_path());
    let mut options = repo
        .checkout_options(gix::worktree::stack::state::attributes::Source::IdMapping)
        .map_err(Error::gitoxide)?;
    options.destination_is_initially_empty = false;
    let should_interrupt = AtomicBool::new(false);
    let files = gix::progress::Discard;
    let bytes = gix::progress::Discard;

    gix::worktree::state::checkout(
        &mut index,
        workdir(repo)?,
        repo.objects.clone().into_arc().map_err(Error::gitoxide)?,
        &files,
        &bytes,
        &should_interrupt,
        options,
    )
    .map_err(Error::gitoxide)?;
    index
        .write(gix::index::write::Options::default())
        .map_err(Error::gitoxide)?;

    use gix::refs::transaction::{Change, LogChange, PreviousValue, RefEdit, RefLog};
    repo.edit_reference(RefEdit {
        change: Change::Update {
            log: LogChange {
                mode: RefLog::AndReference,
                force_create_reflog: false,
                message: format!("checkout: moving to {rev}").into(),
            },
            expected: PreviousValue::Any,
            new: gix::refs::Target::Object(commit),
        },
        name: "HEAD".try_into().map_err(Error::gitoxide)?,
        deref: false,
    })
    .map_err(Error::gitoxide)?;

    Ok(())
}

fn update_submodules(repo: &gix::Repository, terminal_prompt: TerminalPrompt) -> Result<(), Error> {
    let Some(submodules) = repo.submodules().map_err(Error::gitoxide)? else {
        return Ok(());
    };

    for submodule in submodules {
        let Some(rev) = submodule.index_id().map_err(Error::gitoxide)? else {
            continue;
        };
        let path = submodule.work_dir().map_err(Error::gitoxide)?;
        let url = resolve_submodule_url(repo, submodule.url().map_err(Error::gitoxide)?)?;

        debug!("Updating submodule {}", path.display());
        remove_empty_directory(&path)?;
        clone_url_with_fallback(url, &rev.to_string(), &path, terminal_prompt)?;
    }

    Ok(())
}

fn clone_url_attempt(
    url: gix::Url,
    rev: &str,
    path: &Path,
    terminal_prompt: TerminalPrompt,
    shallow: bool,
) -> Result<(), Error> {
    let should_interrupt = AtomicBool::new(false);
    let mut config = vec![
        "protocol.version=2".to_string(),
        format!(
            "gitoxide.credentials.terminalPrompt={}",
            terminal_prompt.credential_prompt_config_value()
        ),
    ];
    if shallow {
        config.push("remote.origin.tagOpt=--no-tags".to_string());
    }

    let mut clone = gix::prepare_clone(url, path)
        .map_err(Error::gitoxide)?
        .with_in_memory_config_overrides(config);
    if shallow {
        clone = clone.with_shallow(gix::remote::fetch::Shallow::DepthAtRemote(
            NonZeroU32::new(1).expect("1 is non-zero"),
        ));
    }
    if !looks_like_object_id(rev) {
        clone = clone.with_ref_name(Some(rev)).map_err(Error::gitoxide)?;
    }

    let (mut checkout, _) = clone
        .fetch_then_checkout(gix::progress::Discard, &should_interrupt)
        .map_err(Error::gitoxide)?;
    let (repo, _) = checkout
        .main_worktree(gix::progress::Discard, &should_interrupt)
        .map_err(Error::gitoxide)?;
    checkout_revision(&repo, rev)?;
    update_submodules(&repo, terminal_prompt)?;

    Ok(())
}

fn clone_url_with_fallback(
    url: gix::Url,
    rev: &str,
    path: &Path,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    if !looks_like_object_id(rev) {
        if let Err(err) = clone_url_attempt(url.clone(), rev, path, terminal_prompt, true) {
            if is_auth_error(&err) {
                warn!(?err, "Failed to shallow clone due to authentication error");
                return Err(err);
            }

            warn!(?err, "Failed to shallow clone, falling back to full clone");
            return clone_url_attempt(url, rev, path, terminal_prompt, false);
        }
    } else {
        clone_url_attempt(url, rev, path, terminal_prompt, false)?;
    }

    Ok(())
}

/// Clone a repository into an initialized destination with the requested terminal prompt mode.
pub(crate) fn clone_repo_at_rev(
    url: &str,
    rev: &str,
    path: &Path,
    terminal_prompt: TerminalPrompt,
) -> Result<(), Error> {
    let url = gix::Url::from_bytes(url.as_bytes().as_bstr()).map_err(Error::gitoxide)?;
    clone_url_with_fallback(url, rev, path, terminal_prompt)
}

pub(crate) fn head_commit(path: &Path) -> Result<String, Error> {
    Ok(discover_repo(path)?
        .head_id()
        .map_err(Error::gitoxide)?
        .detach()
        .to_string())
}

pub(crate) fn remote_head_commit(url: &str) -> Result<String, Error> {
    let tmp = tempfile::tempdir()?;
    let repo = gix::init_bare(tmp.path()).map_err(Error::gitoxide)?;
    let remote = repo.remote_at(url).map_err(Error::gitoxide)?;
    let connection = remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(Error::gitoxide)?;
    let (ref_map, _) = connection
        .ref_map(
            gix::progress::Discard,
            gix::remote::ref_map::Options {
                prefix_from_spec_as_filter_on_remote: false,
                ..Default::default()
            },
        )
        .map_err(Error::gitoxide)?;

    for reference in ref_map.remote_refs {
        match reference {
            gix::protocol::handshake::Ref::Direct {
                full_ref_name,
                object,
            }
            | gix::protocol::handshake::Ref::Symbolic {
                full_ref_name,
                object,
                ..
            } if full_ref_name == "HEAD" => return Ok(object.to_string()),
            gix::protocol::handshake::Ref::Unborn { full_ref_name, .. }
                if full_ref_name == "HEAD" =>
            {
                return Err(Error::gitoxide(std::io::Error::other(
                    "remote HEAD is unborn",
                )));
            }
            _ => {}
        }
    }

    Err(Error::gitoxide(std::io::Error::other(
        "remote HEAD was not advertised",
    )))
}

pub(crate) fn list_remote_tags(url: &str) -> Result<Vec<String>, Error> {
    let tmp = tempfile::tempdir()?;
    let repo = gix::init_bare(tmp.path()).map_err(Error::gitoxide)?;
    let remote = repo.remote_at(url).map_err(Error::gitoxide)?;
    let connection = remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(Error::gitoxide)?;
    let (ref_map, _) = connection
        .ref_map(
            gix::progress::Discard,
            gix::remote::ref_map::Options {
                prefix_from_spec_as_filter_on_remote: false,
                ..Default::default()
            },
        )
        .map_err(Error::gitoxide)?;

    Ok(ref_map
        .remote_refs
        .into_iter()
        .filter_map(|reference| {
            let name = match reference {
                gix::protocol::handshake::Ref::Peeled { full_ref_name, .. }
                | gix::protocol::handshake::Ref::Direct { full_ref_name, .. }
                | gix::protocol::handshake::Ref::Symbolic { full_ref_name, .. }
                | gix::protocol::handshake::Ref::Unborn { full_ref_name, .. } => full_ref_name,
            };
            let name: &[u8] = name.as_ref();
            name.strip_prefix(b"refs/tags/")
                .map(|tag| String::from_utf8_lossy(tag).into_owned())
        })
        .collect())
}

fn get_config_value(
    scope: Option<gix::config::Source>,
    key: &str,
) -> Result<Option<Vec<u8>>, Error> {
    let repo = current_repo()?;
    let config = repo.config_snapshot();
    let config = config.plumbing();
    let value = match scope {
        Some(scope) => config.raw_value_filter(key, |meta| meta.source == scope),
        None => config.raw_value(key),
    };
    Ok(value.ok().map(|value| value.into_owned().into()))
}

fn has_config_value(scope: Option<gix::config::Source>, key: &str) -> Result<bool, Error> {
    // An empty config value still counts as configured and can affect Git's
    // path resolution, e.g. `core.hooksPath=` makes `--git-path hooks`
    // resolve to the current directory.
    Ok(get_config_value(scope, key)?.is_some())
}

fn config_value_is_empty(scope: Option<gix::config::Source>, key: &str) -> Result<bool, Error> {
    Ok(get_config_value(scope, key)?
        .as_deref()
        .is_some_and(<[u8]>::is_empty))
}

pub(crate) fn has_hooks_path_set() -> Result<bool, Error> {
    has_config_value(None, "core.hooksPath")
}

pub(crate) fn global_config_string(key: &str) -> Result<Option<String>, Error> {
    let mut config = gix::config::File::from_globals().map_err(Error::gitoxide)?;
    config.append(gix::config::File::from_environment_overrides().map_err(Error::gitoxide)?);
    Ok(config
        .string(key)
        .map(|value| String::from_utf8_lossy(value.as_ref()).into_owned()))
}

pub(crate) fn has_repo_hooks_path_set() -> Result<bool, Error> {
    Ok(
        has_config_value(Some(gix::config::Source::Local), "core.hooksPath")?
            || has_config_value(Some(gix::config::Source::Worktree), "core.hooksPath")?,
    )
}

/// Compute the file mode for a newly created file based on `core.sharedRepository`.
///
/// This mirrors the relevant parts of Git's `git_config_perm` in `setup.c`
/// and `calc_shared_perm` in `path.c`.
fn shared_repository_file_mode(value: &str, mode: u32) -> Option<u32> {
    const PERM_GROUP: u32 = 0o660;
    const PERM_EVERYBODY: u32 = 0o664;

    fn apply(mode: u32, mut tweak: u32, replace: bool) -> u32 {
        // From Git's `calc_shared_perm`: if the original file is not
        // user-writable, do not introduce any write bits via the shared
        // repository permission tweak.
        if mode & 0o200 == 0 {
            tweak &= !0o222;
        }
        // Also from `calc_shared_perm`: for executable files, mirror read bits
        // into execute bits so an explicit mode like 0640 becomes 0750 when
        // applied to a 0755 file.
        if mode & 0o100 != 0 {
            tweak |= (tweak & 0o444) >> 2;
        }
        // Named values like `group` and `all` add permissions on top of the
        // existing mode, while octal values replace the low permission bits.
        if replace {
            (mode & !0o777) | tweak
        } else {
            mode | tweak
        }
    }

    let value = value.trim().to_ascii_lowercase();
    let (tweak, replace) = match value.as_str() {
        "" | "umask" | "false" | "no" | "off" | "0" => return None,
        "group" | "true" | "yes" | "on" | "1" => (PERM_GROUP, false),
        "all" | "world" | "everybody" | "2" => (PERM_EVERYBODY, false),
        // Parsed like Git's `git_config_perm`, which also accepts explicit
        // octal modes such as `0640`.
        _ => (u32::from_str_radix(&value, 8).ok()?, true),
    };

    // `git_config_perm` rejects explicit modes that do not grant user read/write.
    if replace && tweak & 0o600 != 0o600 {
        return None;
    }

    Some(apply(mode, tweak, replace))
}

/// Resolve the file mode implied by `core.sharedRepository` for a newly created file.
pub(crate) fn get_shared_repository_file_mode(mode: u32) -> Result<u32> {
    if let Some(value) = current_repo()?
        .config_snapshot()
        .string("core.sharedRepository")
    {
        let value = str::from_utf8(value.as_ref())?;
        Ok(shared_repository_file_mode(value, mode).unwrap_or(mode))
    } else {
        Ok(mode)
    }
}

pub(crate) fn tracks_file_mode() -> Result<bool, Error> {
    Ok(repo_tracks_file_mode(&current_repo()?))
}

pub(crate) fn index_stage_output(file_base: &Path) -> Result<Vec<u8>, Error> {
    use std::io::Write as _;

    let repo = current_repo()?;
    let index = open_index(&repo)?;
    let prefix = (!file_base.as_os_str().is_empty()).then(|| {
        let path = gix::path::into_bstr(file_base);
        gix::path::to_unix_separators_on_windows(path.as_ref()).into_owned()
    });

    let mut out = Vec::new();
    for entry in index.entries() {
        let path = entry.path(&index);
        let path_bytes: &[u8] = path.as_ref();
        if let Some(prefix) = prefix.as_deref()
            && !matches_git_prefix(path_bytes, Some(prefix.as_ref()))
        {
            continue;
        }

        write!(
            &mut out,
            "{:06o} {} {}\t",
            entry.mode.bits(),
            entry.id,
            entry.stage_raw()
        )?;
        out.extend_from_slice(path_bytes);
        out.push(0);
    }

    Ok(out)
}

pub(crate) fn get_lfs_files(
    current_dir: &Path,
    paths: &[&Path],
) -> Result<FxHashSet<PathBuf>, Error> {
    if paths.is_empty() {
        return Ok(FxHashSet::default());
    }

    let repo = discover_repo(current_dir)?;
    let index = open_index(&repo)?;
    let mut attributes = repo
        .attributes_only(
            &index,
            gix::worktree::stack::state::attributes::Source::WorktreeThenIdMapping,
        )
        .map_err(Error::gitoxide)?;
    let current_dir = absolute_path(current_dir)?;
    let workdir = workdir(&repo)?.to_path_buf();
    let mut outcome = gix::attrs::search::Outcome::default();
    outcome.initialize_with_selection(
        &gix::attrs::search::MetadataCollection::default(),
        ["filter"],
    );
    let mut lfs_files = FxHashSet::default();

    for path in paths {
        let absolute = if path.is_absolute() {
            path.clean()
        } else {
            current_dir.join(path).clean()
        };
        let Ok(repo_relative) = absolute.strip_prefix(&workdir) else {
            continue;
        };
        let repo_relative = path_to_git_bytes(&repo_relative.clean());
        attributes
            .at_entry(repo_relative.as_slice().as_bstr(), None)
            .map_err(Error::gitoxide)?
            .matching_attributes(&mut outcome);

        if outcome.iter_selected().any(|attribute| {
            matches!(
                attribute.assignment.state,
                gix::attrs::StateRef::Value(value) if value.as_bstr() == "lfs"
            )
        }) {
            lfs_files.insert(path.to_path_buf());
        }
    }

    Ok(lfs_files)
}

/// Check if a git revision exists
pub(crate) fn revision_exists(rev: &str) -> Result<bool, Error> {
    Ok(current_repo()?.rev_parse_single(rev).is_ok())
}

pub(crate) fn object_content(work_dir: &Path, object: &str) -> Result<Vec<u8>, Error> {
    let repo = discover_repo(work_dir)?;
    let id = repo.rev_parse_single(object).map_err(Error::gitoxide)?;
    Ok(repo.find_object(id).map_err(Error::gitoxide)?.data.clone())
}

pub(crate) fn object_size(work_dir: &Path, object: &str) -> Result<u64, Error> {
    Ok(object_content(work_dir, object)?.len() as u64)
}

/// Check if `ancestor` is an ancestor of `commit`.
pub(crate) fn commit_is_ancestor(ancestor: &str, commit: &str) -> Result<bool, Error> {
    let repo = current_repo()?;
    let ancestor = rev_to_commit_id(&repo, ancestor)?;
    let commit = rev_to_commit_id(&repo, commit)?;
    let merge_base = repo
        .merge_base(ancestor, commit)
        .map_err(Error::gitoxide)?
        .detach();
    Ok(merge_base == ancestor)
}

/// Get commits that are ancestors of the given commit but not in the specified remote
pub(crate) fn commits_not_reachable_from_remote(
    local_sha: &str,
    remote_name: &str,
) -> Result<Vec<String>, Error> {
    let repo = current_repo()?;
    let local = repo
        .rev_parse_single(local_sha)
        .map_err(Error::gitoxide)?
        .detach();
    let prefix = format!("refs/remotes/{remote_name}/");
    let hidden = repo
        .references()
        .map_err(Error::gitoxide)?
        .prefixed(prefix.as_str())
        .map_err(Error::gitoxide)?
        .peeled()
        .map_err(Error::gitoxide)?
        .filter_map(|reference| reference.ok()?.try_id().map(gix::Id::detach))
        .collect::<Vec<_>>();
    let mut ancestors = repo
        .rev_walk([local])
        .with_hidden(hidden)
        .all()
        .map_err(Error::gitoxide)?
        .map(|info| {
            info.map(|info| info.id.to_string())
                .map_err(Error::gitoxide)
        })
        .collect::<Result<Vec<_>, _>>()?;
    ancestors.reverse();
    Ok(ancestors)
}

/// Get root commits (commits with no parents) for the given commit
pub(crate) fn root_commits_reachable_from(local_sha: &str) -> Result<FxHashSet<String>, Error> {
    let repo = current_repo()?;
    let local = repo
        .rev_parse_single(local_sha)
        .map_err(Error::gitoxide)?
        .detach();
    repo.rev_walk([local])
        .all()
        .map_err(Error::gitoxide)?
        .filter_map(|info| match info {
            Ok(info) if info.parent_ids.is_empty() => Some(Ok(info.id.to_string())),
            Ok(_) => None,
            Err(err) => Some(Err(Error::gitoxide(err))),
        })
        .collect()
}

/// Get the parent commit of the given commit
pub(crate) fn parent_commit(commit: &str) -> Result<Option<String>, Error> {
    let repo = current_repo()?;
    let Ok(commit) = repo.rev_parse_single(commit) else {
        return Ok(None);
    };
    let commit = repo.find_commit(commit).map_err(Error::gitoxide)?;
    Ok(commit.parent_ids().next().map(|id| id.detach().to_string()))
}

pub(crate) fn current_branch() -> Result<Option<String>, Error> {
    let repo = current_repo()?;
    let Some(name) = repo.head_name().map_err(Error::gitoxide)? else {
        return Ok(None);
    };
    let Some((gix::refs::Category::LocalBranch, branch)) = name.category_and_short_name() else {
        return Ok(None);
    };
    Ok(Some(String::from_utf8_lossy(branch).into_owned()))
}

/// Return a list of absolute paths of all git submodules in the repository.
#[instrument(level = "trace")]
pub(crate) fn list_submodules(git_root: &Path) -> Result<Vec<PathBuf>, Error> {
    let repo = discover_repo(git_root)?;
    let Some(modules) = repo.open_modules_file().map_err(Error::gitoxide)? else {
        return Ok(vec![]);
    };
    modules
        .names()
        .map(|name| {
            modules
                .path(name)
                .map(|path| git_root.join(gix::path::from_bstr(path)))
                .map_err(Error::gitoxide)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::shared_repository_file_mode;
    #[cfg(unix)]
    use super::zsplit;

    #[cfg(unix)]
    #[test]
    fn zsplit_preserves_non_utf8_paths() {
        use std::os::unix::ffi::OsStrExt as _;

        let paths = zsplit(b"normal.py\0bad-\xff.py\0").unwrap();

        assert_eq!(paths.len(), 2);
        assert_eq!(paths[0].as_os_str().as_bytes(), b"normal.py");
        assert_eq!(paths[1].as_os_str().as_bytes(), b"bad-\xff.py");
    }

    #[test]
    fn shared_repository_group_mode_matches_git_behavior() {
        for value in ["group", "true", "yes", "on", "1"] {
            assert_eq!(shared_repository_file_mode(value, 0o755), Some(0o775));
        }
    }

    #[test]
    fn shared_repository_everybody_mode_matches_git_behavior() {
        for value in ["all", "world", "everybody", "2"] {
            assert_eq!(shared_repository_file_mode(value, 0o755), Some(0o775));
        }
    }

    #[test]
    fn shared_repository_octal_mode_matches_git_behavior() {
        assert_eq!(shared_repository_file_mode("0640", 0o644), Some(0o640));
        assert_eq!(shared_repository_file_mode("0640", 0o755), Some(0o750));
    }

    #[test]
    fn shared_repository_umask_or_invalid_values_do_not_override_mode() {
        for value in ["", "umask", "false", "no", "off", "0", "invalid", "0400"] {
            assert_eq!(shared_repository_file_mode(value, 0o755), None);
        }
    }
}
