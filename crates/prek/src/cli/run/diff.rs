use std::path::Path;

use anyhow::Result;

use crate::repo;

/// Tracks project worktree changes across hook priority groups.
pub(crate) struct DiffTracker<'a> {
    path: &'a Path,
    baseline: DiffBaseline,
}

enum DiffBaseline {
    Unknown,
    Snapshot(Vec<u8>),
}

impl<'a> DiffTracker<'a> {
    /// Creates a tracker for a worktree known to have no unstaged changes.
    pub(crate) fn clean_baseline(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Snapshot(Vec::new()),
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
            self.baseline = DiffBaseline::Snapshot(repo::worktree_diff(self.path).await?);
        }
        Ok(())
    }

    /// Checks for worktree changes and advances the tracked baseline.
    pub(crate) async fn changed_after_group(&mut self, track_changes: bool) -> Result<bool> {
        if !track_changes {
            return Ok(false);
        }

        match &mut self.baseline {
            DiffBaseline::Snapshot(prev_diff) => {
                // One patch both detects changes and becomes the next baseline. A
                // separate quiet check would repeat the diff work whenever files are rewritten.
                let curr_diff = repo::worktree_diff(self.path).await?;
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
