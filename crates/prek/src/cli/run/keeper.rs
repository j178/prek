use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use anstream::eprintln;
use anyhow::Result;
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
    async fn clean(root: &Path) -> Result<Self> {
        let files = git::intent_to_add_files(root).await?;
        if files.is_empty() {
            return Ok(Self(vec![]));
        }

        // TODO: xargs
        git_cmd()?
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

    fn restore(&self) -> Result<()> {
        // Restore the intent-to-add changes.
        if !self.0.is_empty() {
            Command::new(GIT.as_ref()?)
                .arg("add")
                .arg("--intent-to-add")
                .arg("--")
                // TODO: xargs
                .args(&self.0)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status()?;
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
            .arg("diff-index")
            .arg("--ignore-submodules")
            .arg("--binary")
            .arg("--exit-code")
            .arg("--no-color")
            .arg("--no-ext-diff")
            .arg(tree)
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
                patch: None,
            })
        } else if output.status.code() == Some(1) {
            if output.stdout.trim_ascii().is_empty() {
                trace!("diff-index status code 1 with empty stdout");
                // probably git auto crlf behavior quirks
                Ok(Self {
                    root: root.to_path_buf(),
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
                        "Unstaged changes detected, stashing unstaged changes to `{}`",
                        patch_path.user_display()
                    )
                    .yellow()
                    .bold()
                );
                fs_err::write(&patch_path, output.stdout)?;

                // Clean the working tree
                debug!("Cleaning working tree");
                Self::checkout_working_tree(root)?;

                Ok(Self {
                    root: root.to_path_buf(),
                    patch: Some(patch_path),
                })
            }
        } else {
            Err(cmd.check_status(output.status).unwrap_err().into())
        }
    }

    fn checkout_working_tree(root: &Path) -> Result<()> {
        let output = Command::new(GIT.as_ref()?)
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
                "Failed to checkout working tree: {output:?}"
            ))
        }
    }

    fn git_apply(patch: &Path) -> Result<()> {
        let output = Command::new(GIT.as_ref()?)
            .arg("apply")
            .arg("--whitespace=nowarn")
            .arg(patch)
            .output()?;
        if output.status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Failed to apply the patch: {output:?}"))
        }
    }

    fn restore(&self) -> Result<()> {
        let Some(patch) = self.patch.as_ref() else {
            return Ok(());
        };

        // Try to apply the patch
        if let Err(e) = Self::git_apply(patch) {
            error!("{e}");
            eprintln!(
                "{}",
                "Stashed changes conflicted with changes made by hook, rolling back the hook changes".red().bold()
            );

            // Discard any changes made by hooks, and try applying the patch again.
            Self::checkout_working_tree(&self.root)?;
            Self::git_apply(patch)?;
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

impl Drop for UnstagedChangesRestorer {
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
    state: Arc<Mutex<Option<WorkTreeState>>>,
}

struct WorkTreeState {
    // Drop order matters: restore file contents before re-adding intent-to-add entries.
    unstaged_changes: UnstagedChangesRestorer,
    intent_to_add: IntentToAddRestorer,
}

impl Drop for WorkTreeKeeper {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap();
        drop(state.take());
    }
}

impl WorkTreeKeeper {
    /// Clear intent-to-add changes from the index and clear the non-staged changes from the working directory.
    /// Restore them when the instance is dropped.
    pub async fn clean(store: &Store, root: &Path) -> Result<Self> {
        let intent_to_add = IntentToAddRestorer::clean(root).await?;
        let unstaged_changes = UnstagedChangesRestorer::clean(root, &store.patches_dir()).await?;
        let state = WorkTreeState {
            unstaged_changes,
            intent_to_add,
        };
        let state = Arc::new(Mutex::new(Some(state)));

        // Make sure restoration when ctrl-c is pressed.
        let cleanup_state = Arc::clone(&state);
        add_cleanup(move || {
            let mut state = cleanup_state.lock().unwrap();
            drop(state.take());
        });

        Ok(Self { state })
    }
}
