use std::path::Path;

use anyhow::Result;

use crate::git;

pub(super) struct DiffTracker<'a> {
    path: &'a Path,
    baseline: DiffBaseline,
    /// Tree object capturing the index when the first potentially-modifying
    /// group runs. All snapshots diff the working tree against this fixed
    /// tree instead of the live index, so a user running `git add` while
    /// hooks execute cannot change the diff — only a hook writing to the
    /// working tree can (see `git::get_tree_diff`).
    baseline_tree: Option<String>,
}

enum DiffBaseline {
    Clean,
    Unknown,
    Snapshot(Vec<u8>),
}

impl<'a> DiffTracker<'a> {
    pub(super) fn clean_baseline(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Clean,
            baseline_tree: None,
        }
    }

    pub(super) fn unknown_baseline(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Unknown,
            baseline_tree: None,
        }
    }

    pub(super) async fn prepare_for_group(&mut self, may_modify_files: bool) -> Result<()> {
        if !may_modify_files {
            return Ok(());
        }
        if self.baseline_tree.is_none() {
            self.baseline_tree = Some(git::write_tree().await?);
        }
        if let DiffBaseline::Unknown = self.baseline {
            let tree = self.baseline_tree.as_deref().expect("tree captured above");
            self.baseline = DiffBaseline::Snapshot(git::get_tree_diff(self.path, tree).await?);
        }
        Ok(())
    }

    pub(super) async fn changed_after_group(
        &mut self,
        may_modify_files: bool,
        all_skipped: bool,
    ) -> Result<bool> {
        // Read-only groups and fully skipped groups cannot change files, so avoid
        // asking git about the working tree.
        if !may_modify_files || all_skipped {
            return Ok(false);
        }

        // `prepare_for_group` ran with `may_modify_files == true`, so the
        // baseline tree exists.
        let tree = self
            .baseline_tree
            .clone()
            .expect("baseline tree captured in prepare_for_group");

        match &mut self.baseline {
            DiffBaseline::Clean => {
                // `WorkTreeKeeper` already removed unstaged changes. A quiet
                // worktree check keeps the common no-op path cheap.
                if !git::has_worktree_diff(self.path).await? {
                    return Ok(false);
                }
                // `diff-files --quiet` is stat-based, so an in-place rewrite
                // can look dirty even when the content is unchanged. Do a full
                // diff here to ignore stat-only changes and reuse the content
                // diff as the baseline if the hook really modified files.
                let curr_diff = git::get_tree_diff(self.path, &tree).await?;
                if curr_diff.is_empty() {
                    return Ok(false);
                }

                // Capture the dirty state after this group so later groups can
                // compare against the exact diff left by previous hooks.
                self.baseline = DiffBaseline::Snapshot(curr_diff);
                Ok(true)
            }
            DiffBaseline::Snapshot(prev_diff) => {
                // Unknown initial state, `--all-files`, and later dirty groups
                // need a full before/after diff comparison to avoid confusing
                // pre-existing user changes with hook changes.
                let curr_diff = git::get_tree_diff(self.path, &tree).await?;
                let modified = curr_diff != *prev_diff;
                *prev_diff = curr_diff;
                Ok(modified)
            }
            DiffBaseline::Unknown => {
                unreachable!("diff baseline must be captured before hooks can modify files")
            }
        }
    }
}
