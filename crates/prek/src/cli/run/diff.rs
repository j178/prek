use std::path::Path;

use anyhow::Result;

use crate::git;

pub(super) struct DiffTracker<'a> {
    path: &'a Path,
    baseline: DiffBaseline,
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
        }
    }

    pub(super) fn unknown_baseline(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Unknown,
        }
    }

    pub(super) async fn prepare_for_group(&mut self, may_modify_files: bool) -> Result<()> {
        if may_modify_files && let DiffBaseline::Unknown = self.baseline {
            self.baseline = DiffBaseline::Snapshot(git::get_diff(self.path).await?);
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
                let curr_diff = git::get_diff(self.path).await?;
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
                let curr_diff = git::get_diff(self.path).await?;
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
