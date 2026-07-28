use std::path::Path;

use anyhow::Result;

use crate::git;

/// Tracks project worktree changes across hook priority groups.
pub(crate) struct DiffTracker<'a> {
    path: &'a Path,
    baseline: DiffBaseline,
}

enum DiffBaseline {
    Clean,
    Unknown,
    Snapshot(Vec<u8>),
}

impl<'a> DiffTracker<'a> {
    /// Creates a tracker for a worktree known to have no unstaged changes.
    pub(crate) fn clean_baseline(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Clean,
        }
    }

    /// Creates a tracker whose initial worktree state is unknown.
    pub(crate) fn unknown_baseline(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Unknown,
        }
    }

    /// Captures an unknown baseline before a group that requires diff tracking.
    pub(crate) async fn prepare_for_group(&mut self, track_changes: bool) -> Result<()> {
        if track_changes && let DiffBaseline::Unknown = self.baseline {
            self.baseline = DiffBaseline::Snapshot(git::diff_worktree(self.path).await?);
        }
        Ok(())
    }

    /// Checks for worktree changes and advances the tracked baseline.
    pub(crate) async fn changed_after_group(&mut self, track_changes: bool) -> Result<bool> {
        if !track_changes {
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
                let curr_diff = git::diff_worktree(self.path).await?;
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
                let curr_diff = git::diff_worktree(self.path).await?;
                let modified = curr_diff != *prev_diff;
                *prev_diff = curr_diff;
                Ok(modified)
            }
            DiffBaseline::Unknown => {
                unreachable!("diff baseline must be captured before hooks can modify files")
            }
        }
    }

    /// Discards the baseline after a hook reports a modification directly.
    pub(crate) fn invalidate(&mut self) {
        self.baseline = DiffBaseline::Unknown;
    }
}
