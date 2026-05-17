use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anstream::eprintln;
use anyhow::Result;
use owo_colors::OwoColorize;
use tracing::debug;

use crate::cleanup::add_cleanup;
use crate::fs::Simplified;
use crate::git;
use crate::store::Store;

static RESTORE_WORKTREE: Mutex<Option<WorkTreeKeeper>> = Mutex::new(None);

struct IntentToAddKeeper {
    root: PathBuf,
    files: Vec<PathBuf>,
}
struct WorkingTreeKeeper {
    root: PathBuf,
    patch: Option<PathBuf>,
    snapshots: Vec<WorktreeSnapshot>,
}

#[derive(Clone, Eq, PartialEq)]
struct WorktreeSnapshot {
    path: PathBuf,
    clean: SnapshotContent,
    unstaged: SnapshotContent,
}

#[derive(Clone, Eq, PartialEq)]
enum SnapshotContent {
    Missing,
    File { content: Vec<u8>, executable: bool },
    Symlink(PathBuf),
}

fn ensure_patches_dir(path: &Path) -> Result<()> {
    fs_err::create_dir_all(path)?;

    #[cfg(unix)]
    {
        use std::fs::Permissions;
        use std::os::unix::fs::PermissionsExt;

        // Patch files can contain unstaged source diffs, so keep the directory owner-only.
        let _ = fs_err::set_permissions(path, Permissions::from_mode(0o700));
    }

    Ok(())
}

fn remove_path(path: &Path) -> Result<()> {
    match fs_err::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            fs_err::remove_dir_all(path)?;
        }
        Ok(_) => {
            fs_err::remove_file(path)?;
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }
    Ok(())
}

#[cfg(unix)]
fn is_executable(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &std::fs::Metadata) -> bool {
    false
}

fn read_snapshot(path: &Path) -> Result<SnapshotContent> {
    match fs_err::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            Ok(SnapshotContent::Symlink(fs_err::read_link(path)?))
        }
        Ok(metadata) if metadata.is_file() => Ok(SnapshotContent::File {
            content: fs_err::read(path)?,
            executable: is_executable(&metadata),
        }),
        Ok(metadata) if metadata.is_dir() => Ok(SnapshotContent::Missing),
        Ok(_) => Ok(SnapshotContent::Missing),
        Err(err) if err.kind() == ErrorKind::NotFound => Ok(SnapshotContent::Missing),
        Err(err) => Err(err.into()),
    }
}

fn write_snapshot(path: &Path, content: &SnapshotContent) -> Result<()> {
    remove_path(path)?;

    let Some(parent) = path.parent() else {
        return Ok(());
    };

    match content {
        SnapshotContent::Missing => {}
        SnapshotContent::File {
            content,
            executable,
        } => {
            fs_err::create_dir_all(parent)?;
            fs_err::write(path, content)?;

            #[cfg(unix)]
            {
                use std::fs::Permissions;
                use std::os::unix::fs::PermissionsExt as _;

                let mode = if *executable { 0o755 } else { 0o644 };
                fs_err::set_permissions(path, Permissions::from_mode(mode))?;
            }
        }
        SnapshotContent::Symlink(target) => {
            fs_err::create_dir_all(parent)?;
            #[cfg(unix)]
            {
                std::os::unix::fs::symlink(target, path)?;
            }
            #[cfg(not(unix))]
            {
                fs_err::write(path, target.as_os_str().as_encoded_bytes())?;
            }
        }
    }

    Ok(())
}

impl IntentToAddKeeper {
    fn clean(root: &Path) -> Result<Self> {
        let files = git::clear_intent_to_add_files(root)?;
        Ok(Self {
            root: root.to_path_buf(),
            files,
        })
    }

    fn restore(&self) -> Result<()> {
        git::restore_intent_to_add_files(&self.root, &self.files)?;
        Ok(())
    }
}

impl Drop for IntentToAddKeeper {
    fn drop(&mut self) {
        if let Err(err) = self.restore() {
            eprintln!(
                "{}",
                format!("Failed to restore intent-to-add changes: {err}").red()
            );
        }
    }
}

impl WorkingTreeKeeper {
    fn clean(root: &Path, patch_dir: &Path) -> Result<Self> {
        let files = git::files_not_staged_under(root)?;
        if files.is_empty() {
            debug!("Working tree is clean");
            return Ok(Self {
                root: root.to_path_buf(),
                patch: None,
                snapshots: Vec::new(),
            });
        }

        let now = std::time::SystemTime::now();
        let pid = std::process::id();
        let patch_name = format!(
            "{}-{}.patch",
            now.duration_since(std::time::UNIX_EPOCH)?.as_millis(),
            pid
        );
        ensure_patches_dir(patch_dir)?;
        let patch_path = patch_dir.join(&patch_name);

        debug!("Unstaged changes detected");
        eprintln!(
            "{}",
            format!(
                "Unstaged changes detected, stashing unstaged changes to `{}`",
                patch_path.user_display()
            )
            .yellow()
            .bold()
        );

        let mut snapshots = files
            .iter()
            .map(|path| {
                Ok(WorktreeSnapshot {
                    path: path.clone(),
                    clean: SnapshotContent::Missing,
                    unstaged: read_snapshot(&root.join(path))?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        fs_err::write(&patch_path, git::worktree_diff(root, false)?)?;

        debug!("Cleaning working tree");
        git::restore_index_paths(root, &files)?;
        for snapshot in &mut snapshots {
            snapshot.clean = read_snapshot(&root.join(&snapshot.path))?;
        }

        Ok(Self {
            root: root.to_path_buf(),
            patch: Some(patch_path),
            snapshots,
        })
    }

    fn restore(&self) -> Result<()> {
        let Some(patch) = self.patch.as_ref() else {
            return Ok(());
        };

        let changed_by_hooks = self.snapshots.iter().filter_map(|snapshot| {
            let path = self.root.join(&snapshot.path);
            (read_snapshot(&path).ok()? != snapshot.clean).then(|| snapshot.path.clone())
        });
        let changed_by_hooks = changed_by_hooks.collect::<Vec<_>>();
        if !changed_by_hooks.is_empty() {
            eprintln!(
                "{}",
                "Stashed changes conflicted with changes made by hook, rolling back the hook changes"
                    .red()
                    .bold()
            );
            git::restore_index_paths(&self.root, &changed_by_hooks)?;
        }

        for snapshot in &self.snapshots {
            write_snapshot(&self.root.join(&snapshot.path), &snapshot.unstaged)?;
        }

        eprintln!(
            "{}",
            format!(
                "Restored working tree changes from `{}`",
                patch.user_display()
            )
            .yellow()
            .bold()
        );

        Ok(())
    }
}

impl Drop for WorkingTreeKeeper {
    fn drop(&mut self) {
        if let Err(err) = self.restore() {
            eprintln!(
                "{}",
                format!("Failed to restore working tree changes: {err}").red()
            );
        }
    }
}

/// Clean Git intent-to-add files and working tree changes, and restore them when dropped.
pub struct WorkTreeKeeper {
    intent_to_add: Option<IntentToAddKeeper>,
    working_tree: Option<WorkingTreeKeeper>,
}

#[derive(Default)]
pub struct RestoreGuard {
    _guard: (),
}

impl Drop for RestoreGuard {
    fn drop(&mut self) {
        if let Some(mut keeper) = RESTORE_WORKTREE.lock().unwrap().take() {
            keeper.restore();
        }
    }
}

impl WorkTreeKeeper {
    /// Clear intent-to-add changes from the index and clear the non-staged changes from the working directory.
    /// Restore them when the instance is dropped.
    pub fn clean(store: &Store, root: &Path) -> Result<RestoreGuard> {
        let cleaner = Self {
            intent_to_add: Some(IntentToAddKeeper::clean(root)?),
            working_tree: Some(WorkingTreeKeeper::clean(root, &store.patches_dir())?),
        };

        // Set to the global for the cleanup hook.
        *RESTORE_WORKTREE.lock().unwrap() = Some(cleaner);

        // Make sure restoration when ctrl-c is pressed.
        add_cleanup(|| {
            if let Some(guard) = &mut *RESTORE_WORKTREE.lock().unwrap() {
                guard.restore();
            }
        });

        Ok(RestoreGuard::default())
    }

    /// Restore the intent-to-add changes and non-staged changes.
    fn restore(&mut self) {
        self.intent_to_add.take();
        self.working_tree.take();
    }
}
