use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;

use anstream::eprintln;
use anyhow::Result;
use owo_colors::OwoColorize;
use tracing::{error, trace};

use constants::env_vars::EnvVars;

use crate::cleanup::add_cleanup;
use crate::fs::Simplified;
use crate::git::{self, GIT, git_cmd};
use crate::store::Store;

static RESTORE_WORKTREE: Mutex<Option<WorkTreeKeeper>> = Mutex::new(None);

struct IntentToAddKeeper(Vec<PathBuf>);
struct WorkingTreeKeeper(Option<PathBuf>);

impl IntentToAddKeeper {
    async fn clean() -> Result<Self> {
        let files = git::intent_to_add_files().await?;
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

        Ok(Self(files.into_iter().map(PathBuf::from).collect()))
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
            anstream::eprintln!(
                "{}",
                format!("Failed to restore intent-to-add changes: {err}").red()
            );
        }
    }
}

impl WorkingTreeKeeper {
    async fn clean(patch_dir: &Path) -> Result<Self> {
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
            .check(false)
            .output()
            .await?;

        if output.status.success() {
            trace!("No non-staged changes detected");
            // No non-staged changes
            Ok(Self(None))
        } else if output.status.code() == Some(1) {
            if output.stdout.trim_ascii().is_empty() {
                trace!("diff-index status code 1 with empty stdout");
                // probably git auto crlf behavior quirks
                Ok(Self(None))
            } else {
                let now = std::time::SystemTime::now();
                let pid = std::process::id();
                let patch_name = format!(
                    "{}-{}.patch",
                    now.duration_since(std::time::UNIX_EPOCH)?.as_millis(),
                    pid
                );
                let patch_path = patch_dir.join(&patch_name);

                eprintln!(
                    "{}",
                    format!(
                        "Non-staged changes detected, saving to `{}`",
                        patch_path.user_display()
                    )
                    .yellow()
                );
                fs_err::create_dir_all(patch_dir)?;
                fs_err::write(&patch_path, output.stdout)?;

                // Clean the working tree
                Self::checkout_working_tree()?;

                Ok(Self(Some(patch_path)))
            }
        } else {
            Err(cmd.check_status(output.status).unwrap_err().into())
        }
    }

    fn checkout_working_tree() -> Result<()> {
        let status = Command::new(GIT.as_ref()?)
            .arg("-c")
            .arg("submodule.recurse=0")
            .arg("checkout")
            .arg("--")
            .arg(".")
            // prevent recursive post-checkout hooks
            .env(EnvVars::PREFLIGIT_INTERNAL__SKIP_POST_CHECKOUT, "1")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Failed to checkout working tree"))
        }
    }

    fn git_apply(patch: &Path) -> Result<()> {
        let status = Command::new(GIT.as_ref()?)
            .arg("apply")
            .arg("--whitespace=nowarn")
            .arg(patch)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(anyhow::anyhow!("Failed to apply the patch"))
        }
    }

    fn restore(&self) -> Result<()> {
        let Some(patch) = self.0.as_ref() else {
            return Ok(());
        };

        // Try to apply the patch
        if Self::git_apply(patch).is_err() {
            error!("Failed to apply the patch, rolling back changes");
            eprintln!(
                "{}",
                "Failed to apply the patch, rolling back changes".red()
            );

            Self::checkout_working_tree()?;
            Self::git_apply(patch)?;
        }

        eprintln!(
            "{}",
            format!(
                "\nRestored working tree changes from `{}`",
                patch.user_display()
            )
            .yellow()
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
    pub async fn clean(store: &Store) -> Result<RestoreGuard> {
        let cleaner = Self {
            intent_to_add: Some(IntentToAddKeeper::clean().await?),
            working_tree: Some(WorkingTreeKeeper::clean(&store.patches_dir()).await?),
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
