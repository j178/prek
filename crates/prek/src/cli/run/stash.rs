use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anstream::eprintln;
use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use prek_consts::env_vars::EnvVars;
use tracing::{debug, error, trace};

use crate::cleanup::add_cleanup;
use crate::fs::Simplified;
use crate::git::{self, git_cmd};
use crate::process::Cmd;
use crate::store::Store;

struct SavedPatch {
    root: PathBuf,
    tree: String,
    path: PathBuf,
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

fn git_command() -> Result<Cmd> {
    let mut cmd = git_cmd()?;
    cmd.current_dir(git::root()?);
    Ok(cmd)
}

impl SavedPatch {
    fn save(root: &Path, patch_dir: &Path) -> Result<Option<Self>> {
        let tree = git::write_tree()?;

        let output = git_command()?
            .arg("diff-index")
            .arg("--binary")
            .arg("--exit-code")
            .hidden_args([
                "--ignore-submodules",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--no-relative",
            ])
            .arg(&tree)
            .arg("--")
            .arg(root)
            .check(false)
            .output_sync()?;

        match output.status.code() {
            Some(0) => {
                debug!("Working tree is clean");
                return Ok(None);
            }
            Some(1) if output.stdout.trim_ascii().is_empty() => {
                trace!("diff-index status code 1 with empty stdout");
                // Git can report CRLF-only differences without producing a patch.
                return Ok(None);
            }
            Some(1) => {}
            _ => anyhow::bail!(
                "Failed to save unstaged changes:\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ),
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

        eprintln!(
            "{}",
            format!(
                "Unstaged changes detected. Temporarily saving them to `{}`",
                patch_path.user_display()
            )
            .yellow()
            .bold()
        );
        let mut patch_file = fs_err::File::create(&patch_path)?;
        // Keep the baseline discoverable even if the run's log is overwritten.
        writeln!(patch_file, "# prek pre-hook index tree: {tree}")?;
        patch_file.write_all(&output.stdout)?;

        Ok(Some(Self {
            root: root.to_path_buf(),
            tree,
            path: patch_path,
        }))
    }

    fn checkout(&self) -> Result<()> {
        git_command()?
            .args(["-c", "submodule.recurse=0", "checkout", "--"])
            .arg(&self.root)
            // Prevent recursive post-checkout hooks.
            .env(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT, "1")
            .output_sync()
            .context("Failed to checkout working tree")?;
        Ok(())
    }

    fn restore_from_tree(&self) -> Result<()> {
        // Unstage hook additions without deleting files that were previously untracked.
        git_command()?
            .args(["reset", "--quiet", &self.tree, "--"])
            .arg(&self.root)
            .output_sync()
            .context("Failed to restore the pre-hook index")?;
        self.checkout()?;
        self.apply()
    }

    fn apply(&self) -> Result<()> {
        git_command()?
            .args(["apply", "--whitespace=nowarn"])
            .arg(&self.path)
            .output_sync()
            .context("Failed to apply the patch")?;
        Ok(())
    }

    fn restore(&self) -> Result<RestoreOutcome> {
        let outcome = if let Err(e) = self.apply() {
            error!("{e}");
            eprintln!(
                "{}",
                "Hook changes conflicted with the saved unstaged changes. Reverting the hook changes".red().bold()
            );

            self.restore_from_tree().with_context(|| {
                format!(
                    "Failed to restore unstaged changes.\n\
                     Your changes are saved in `{}`.\n\
                     Pre-hook index tree: {}",
                    self.path.user_display(),
                    self.tree,
                )
            })?;
            RestoreOutcome::HookChangesReverted
        } else {
            RestoreOutcome::Restored
        };

        eprintln!(
            "{}",
            format!(
                "Restored unstaged changes from `{}`",
                self.path.user_display()
            )
            .yellow()
            .bold()
        );

        Ok(outcome)
    }
}

pub(super) enum RestoreOutcome {
    Restored,
    HookChangesReverted,
}

/// Temporarily save unstaged changes, restoring them explicitly or when dropped.
pub(super) struct WorktreeStash {
    // Preparation and restoration share the lock so cleanup cannot race a Git mutation.
    // None means restoration has already been attempted, even if it failed.
    state: Arc<Mutex<Option<PendingChanges>>>,
}

struct PendingChanges {
    intent_to_add: Vec<PathBuf>,
    // Recorded before checkout, so a failed checkout is also recoverable.
    patch: Option<SavedPatch>,
}

impl PendingChanges {
    fn prepare(&mut self, root: &Path, patch_dir: &Path) -> Result<()> {
        if !self.intent_to_add.is_empty() {
            git_command()?
                .args(["rm", "--cached", "--"])
                .args(&self.intent_to_add)
                .output_sync()
                .context("Failed to clear intent-to-add changes")?;
        }
        self.patch = SavedPatch::save(root, patch_dir)?;
        if let Some(patch) = &self.patch {
            debug!("Cleaning working tree");
            patch.checkout()?;
        }
        Ok(())
    }

    fn restore(self) -> Result<RestoreOutcome> {
        let unstaged = match &self.patch {
            Some(patch) => patch.restore(),
            None => Ok(RestoreOutcome::Restored),
        };
        // Restore file contents before intent-to-add markers. Attempt both even if one fails.
        let intent = self.restore_intent();
        match (unstaged, intent) {
            (result, Ok(())) => result,
            (Ok(_), Err(err)) => Err(err),
            (Err(err), Err(intent_err)) => {
                Err(anyhow::anyhow!("{err:#}\n\nAdditionally:\n{intent_err:#}"))
            }
        }
    }

    fn restore_intent(&self) -> Result<()> {
        if !self.intent_to_add.is_empty() {
            git_command()?
                .args(["add", "--intent-to-add", "--"])
                .args(&self.intent_to_add)
                .output_sync()
                .context("Failed to restore intent-to-add changes")?;
        }
        Ok(())
    }
}

impl Drop for WorktreeStash {
    fn drop(&mut self) {
        if let Err(err) = Self::restore_once(&self.state) {
            eprintln!("{}", format!("{err:#}").red());
        }
    }
}

impl WorktreeStash {
    /// Restore saved changes and report whether hook changes had to be reverted.
    /// Failed restoration is not retried when this stash is dropped.
    pub fn restore(self) -> Result<RestoreOutcome> {
        Self::restore_once(&self.state)
    }

    fn restore_once(state: &Mutex<Option<PendingChanges>>) -> Result<RestoreOutcome> {
        // Keep cleanup on another thread from exiting before restoration finishes.
        let mut guard = state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(state) = guard.take() else {
            return Ok(RestoreOutcome::Restored);
        };
        state.restore()
    }

    /// Save unstaged changes and intent-to-add markers, then prepare the worktree for hooks.
    ///
    /// Intent-to-add paths must be absolute; only paths under `root` are cleared.
    pub fn save(store: &Store, root: &Path, mut intent_to_add: Vec<PathBuf>) -> Result<Self> {
        intent_to_add.retain(|path| path.starts_with(root));
        let stash = Self {
            state: Arc::new(Mutex::new(Some(PendingChanges {
                intent_to_add,
                patch: None,
            }))),
        };
        let state = Arc::clone(&stash.state);
        add_cleanup(move || {
            if let Err(err) = Self::restore_once(&state) {
                eprintln!("{}", format!("{err:#}").red());
            }
        });

        let result = {
            // Even write-tree can update the index cache. Finish preparation before allowing
            // Ctrl-C cleanup to restore it.
            let mut guard = stash
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let state = guard.as_mut().context("Worktree cleanup was interrupted")?;
            state.prepare(root, &store.patches_dir())
        };
        if let Err(err) = result {
            return match stash.restore() {
                Ok(_) => Err(err),
                Err(restore_err) => Err(anyhow::anyhow!(
                    "{err:#}\n\nRecovery also failed:\n{restore_err:#}"
                )),
            };
        }
        Ok(stash)
    }
}
