use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anstream::eprintln;
use anyhow::Result;
use owo_colors::OwoColorize;
use tracing::{debug, error, trace};

use prek_consts::env_vars::EnvVars;

use crate::cleanup::add_cleanup;
use crate::fs::Simplified;
use crate::git::{self, GIT, git_cmd};
use crate::store::Store;

static RESTORE_WORKTREE: Mutex<Option<WorkTreeKeeper>> = Mutex::new(None);

struct IntentToAddKeeper(Vec<PathBuf>);
struct WorkingTreeKeeper {
    root: PathBuf,
    patch: Option<PathBuf>,
}

impl IntentToAddKeeper {
    async fn clean(root: &Path) -> Result<Self> {
        let files = git::intent_to_add_files(root).await?;
        if files.is_empty() {
            return Ok(Self(vec![]));
        }

        // TODO: xargs
        git_cmd("git rm")?
            .arg("rm")
            .arg("--cached")
            .arg("--")
            .args(&files)
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
    async fn clean(root: &Path, patch_dir: &Path) -> Result<Self> {
        let tree = git::write_tree().await?;

        let mut cmd = git_cmd("git diff-index")?;
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
                fs_err::create_dir_all(patch_dir)?;
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
    pub async fn clean(store: &Store, root: &Path) -> Result<RestoreGuard> {
        let cleaner = Self {
            intent_to_add: Some(IntentToAddKeeper::clean(root).await?),
            working_tree: Some(WorkingTreeKeeper::clean(root, &store.patches_dir()).await?),
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
