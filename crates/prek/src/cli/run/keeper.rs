use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anstream::eprintln;
use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use prek_consts::env_vars::EnvVars;
use tracing::{debug, error, trace};

use crate::cleanup::add_cleanup;
use crate::fs::Simplified;
use crate::git::{self, GIT, git_cmd};
use crate::store::Store;

struct IntentToAddRestorer(Vec<PathBuf>);
struct UnstagedChangesRestorer {
    root: PathBuf,
    tree: String,
    patch: Option<PathBuf>,
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

impl IntentToAddRestorer {
    async fn clean(root: &Path, mut files: Vec<PathBuf>) -> Result<Self> {
        files.retain(|path| path.starts_with(root));
        if files.is_empty() {
            return Ok(Self(vec![]));
        }

        // TODO: xargs
        git_cmd()?
            .current_dir(git::root()?)
            .arg("rm")
            .arg("--cached")
            .arg("--")
            .file_args(&files)
            .check(true)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await?;

        Ok(Self(files))
    }

    fn restore(&mut self) -> Result<()> {
        // Restore the intent-to-add changes.
        let files = std::mem::take(&mut self.0);
        if !files.is_empty() {
            let mut cmd = Command::new(GIT.as_ref()?);
            let output = git::apply_git_work_tree(&mut cmd)
                .current_dir(git::root()?)
                .arg("add")
                .arg("--intent-to-add")
                .arg("--")
                // TODO: xargs
                .args(&files)
                .output()?;
            if !output.status.success() {
                anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
            }
        }
        Ok(())
    }
}

impl Drop for IntentToAddRestorer {
    fn drop(&mut self) {
        if let Err(err) = self.restore() {
            eprintln!(
                "{}",
                format!("Failed to restore intent-to-add changes: {err}").red()
            );
        }
    }
}

impl UnstagedChangesRestorer {
    async fn clean(root: &Path, patch_dir: &Path) -> Result<Self> {
        let tree = git::write_tree().await?;

        let mut cmd = git_cmd()?;
        let output = cmd
            .current_dir(git::root()?)
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
            .output()
            .await?;

        if output.status.success() {
            debug!("Working tree is clean");
            // No non-staged changes
            Ok(Self {
                root: root.to_path_buf(),
                tree,
                patch: None,
            })
        } else if output.status.code() == Some(1) {
            if output.stdout.trim_ascii().is_empty() {
                trace!("diff-index status code 1 with empty stdout");
                // probably git auto crlf behavior quirks
                Ok(Self {
                    root: root.to_path_buf(),
                    tree,
                    patch: None,
                })
            } else {
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
                drop(patch_file);

                let restorer = Self {
                    root: root.to_path_buf(),
                    tree,
                    patch: Some(patch_path),
                };

                // Clean the working tree
                debug!("Cleaning working tree");
                Self::checkout_working_tree(root)?;

                Ok(restorer)
            }
        } else {
            Err(cmd.check_status(output.status).unwrap_err().into())
        }
    }

    fn checkout_working_tree(root: &Path) -> Result<()> {
        let mut cmd = Command::new(GIT.as_ref()?);
        let output = git::apply_git_work_tree(&mut cmd)
            .current_dir(git::root()?)
            .arg("-c")
            .arg("submodule.recurse=0")
            .arg("checkout")
            .arg("--")
            .arg(root)
            // prevent recursive post-checkout hooks
            .env(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT, "1")
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to checkout working tree:\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn rollback_hook_changes(&self) -> Result<()> {
        // Unstage hook additions without deleting files that were previously untracked.
        let mut cmd = Command::new(GIT.as_ref()?);
        let output = git::apply_git_work_tree(&mut cmd)
            .current_dir(git::root()?)
            .args(["reset", "--quiet", &self.tree, "--"])
            .arg(&self.root)
            .output()?;
        if !output.status.success() {
            anyhow::bail!(
                "Failed to restore the pre-hook index:\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Self::checkout_working_tree(&self.root)
    }

    fn git_apply(patch: &Path) -> Result<()> {
        let mut cmd = Command::new(GIT.as_ref()?);
        let output = git::apply_git_work_tree(&mut cmd)
            .current_dir(git::root()?)
            .arg("apply")
            .arg("--whitespace=nowarn")
            .arg(patch)
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!(
                "Failed to apply the patch:\n{}",
                String::from_utf8_lossy(&output.stderr).trim()
            ))
        }
    }

    fn restore(&mut self) -> Result<bool> {
        let Some(patch) = self.patch.take() else {
            return Ok(false);
        };

        self.restore_patch(&patch).with_context(|| {
            format!(
                "Failed to restore unstaged changes.\n\
                 Your changes are saved in `{}`.\n\
                 Pre-hook index tree: {}\n\
                 To recover in a separate directory, see https://prek.j178.dev/debugging/#recovering-unstaged-changes",
                patch.user_display(),
                self.tree,
            )
        })
    }

    fn restore_patch(&self, patch: &Path) -> Result<bool> {
        let mut rolled_back = false;
        // Try to apply the patch
        if let Err(e) = Self::git_apply(patch) {
            error!("{e}");
            eprintln!(
                "{}",
                "Hook changes conflicted with the saved unstaged changes. Reverting the hook changes".red().bold()
            );

            self.rollback_hook_changes()?;
            Self::git_apply(patch)?;
            rolled_back = true;
        }

        eprintln!(
            "{}",
            format!("Restored unstaged changes from `{}`", patch.user_display())
                .yellow()
                .bold()
        );

        Ok(rolled_back)
    }
}

impl Drop for UnstagedChangesRestorer {
    fn drop(&mut self) {
        if let Err(err) = self.restore() {
            eprintln!("{}", format!("{err:#}").red());
        }
    }
}

/// Clean Git intent-to-add files and working tree changes, and restore them when dropped.
pub struct WorkTreeKeeper {
    state: Arc<Mutex<Option<WorkTreeState>>>,
}

struct WorkTreeState {
    // Drop order matters: restore file contents before re-adding intent-to-add entries.
    unstaged_changes: UnstagedChangesRestorer,
    intent_to_add: IntentToAddRestorer,
}

impl Drop for WorkTreeKeeper {
    fn drop(&mut self) {
        if let Err(err) = self.restore() {
            eprintln!("{}", format!("{err:#}").red());
        }
    }
}

impl WorkTreeKeeper {
    /// Restore saved changes, returning whether hook changes were rolled back.
    pub fn restore(&self) -> Result<bool> {
        // Keep cleanup on another thread from exiting before restoration finishes.
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let Some(mut state) = guard.take() else {
            return Ok(false);
        };
        let rolled_back = state.unstaged_changes.restore()?;
        state
            .intent_to_add
            .restore()
            .context("Failed to restore intent-to-add changes")?;
        Ok(rolled_back)
    }

    /// Clear intent-to-add changes from the index and clear the non-staged changes from the working directory.
    /// Restore them when the instance is dropped.
    /// Intent-to-add paths must be absolute; only paths under `root` are cleared.
    pub async fn clean(store: &Store, root: &Path, intent_to_add: Vec<PathBuf>) -> Result<Self> {
        let intent_to_add = IntentToAddRestorer::clean(root, intent_to_add).await?;
        let unstaged_changes = UnstagedChangesRestorer::clean(root, &store.patches_dir()).await?;
        let state = WorkTreeState {
            unstaged_changes,
            intent_to_add,
        };
        let state = Arc::new(Mutex::new(Some(state)));

        // Make sure restoration when ctrl-c is pressed.
        let cleanup_keeper = Self {
            state: Arc::clone(&state),
        };
        add_cleanup(move || {
            if let Err(err) = cleanup_keeper.restore() {
                eprintln!("{}", format!("{err:#}").red());
            }
        });

        Ok(Self { state })
    }
}
