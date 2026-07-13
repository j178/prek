use std::path::Path;

use anyhow::Result;

use crate::git;

pub(super) struct DiffTracker<'a> {
    path: &'a Path,
    baseline: DiffBaseline,
    /// When set, detect modifications from working-tree content (via
    /// `git::get_working_tree_diff`) instead of the live-index diff, so a user
    /// concurrently staging or unstaging files is not misread as a hook
    /// modification. Set by `run --working-tree`.
    working_tree: bool,
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
            working_tree: false,
        }
    }

    pub(super) fn unknown_baseline(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Unknown,
            working_tree: false,
        }
    }

    /// Track modifications by working-tree content rather than the live index.
    ///
    /// Always uses the before/after snapshot comparison (never the index-based
    /// `Clean` fast path, whose `diff-files` check is itself index-sensitive).
    pub(super) fn working_tree(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Unknown,
            working_tree: true,
        }
    }

    /// The before/after snapshot: working-tree content vs `HEAD` in
    /// `--working-tree` mode, otherwise the working tree vs the live index.
    ///
    /// An associated function over the individual fields (rather than `&self`)
    /// so it can be called while `self.baseline` is mutably borrowed.
    async fn snapshot(path: &Path, working_tree: bool) -> Result<Vec<u8>> {
        if working_tree {
            Ok(git::get_working_tree_diff(path).await?)
        } else {
            Ok(git::get_diff(path).await?)
        }
    }

    pub(super) async fn prepare_for_group(&mut self, may_modify_files: bool) -> Result<()> {
        if may_modify_files && let DiffBaseline::Unknown = self.baseline {
            self.baseline =
                DiffBaseline::Snapshot(Self::snapshot(self.path, self.working_tree).await?);
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
                let curr_diff = Self::snapshot(self.path, self.working_tree).await?;
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
                let curr_diff = Self::snapshot(self.path, self.working_tree).await?;
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
