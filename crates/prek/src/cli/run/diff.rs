use std::path::Path;

use anyhow::Result;
use tracing::debug;

use crate::git;

pub(super) struct DiffTracker<'a> {
    path: &'a Path,
    baseline: DiffBaseline,
    /// Tree object capturing the index when the first potentially-modifying
    /// group runs. All snapshots diff the working tree against this fixed
    /// tree instead of the live index, so a user running `git add` while
    /// hooks execute cannot change the diff — only a hook writing to the
    /// working tree can (see `git::get_tree_diff`).
    ///
    /// Stays `None` when `git write-tree` cannot serialize the index (an
    /// unmerged or partially-present index); snapshots then fall back to the
    /// live-index diff, preserving the best-effort behaviour hooks had before
    /// tree anchoring.
    baseline_tree: Option<String>,
    /// Whether we have already attempted to capture `baseline_tree`, so a
    /// failing `write-tree` is not retried before every group.
    ///
    /// Not retrying is safe because the failures that remain are properties of
    /// the index, not of timing: `git::write_tree` builds its tree from a
    /// private copy, so a concurrent `git add` holding `index.lock` no longer
    /// fails the capture. A retry would only re-derive the same answer, and the
    /// two are indistinguishable anyway — both exit 128, and their messages are
    /// localized.
    baseline_tree_captured: bool,
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
            baseline_tree_captured: false,
        }
    }

    pub(super) fn unknown_baseline(path: &'a Path) -> Self {
        Self {
            path,
            baseline: DiffBaseline::Unknown,
            baseline_tree: None,
            baseline_tree_captured: false,
        }
    }

    pub(super) async fn prepare_for_group(&mut self, may_modify_files: bool) -> Result<()> {
        if !may_modify_files {
            return Ok(());
        }
        if !self.baseline_tree_captured {
            // Best-effort: `write-tree` fails on an unmerged or partially-present
            // index. Leave `baseline_tree` as `None` in that case so snapshots
            // fall back to the live-index diff (see `git::get_tree_diff`).
            self.baseline_tree = git::write_tree()
                .await
                .inspect_err(|err| {
                    debug!("Falling back to live-index diffs, cannot write baseline tree: {err}");
                })
                .ok();
            self.baseline_tree_captured = true;
        }
        if let DiffBaseline::Unknown = self.baseline {
            self.baseline = DiffBaseline::Snapshot(
                git::get_tree_diff(self.path, self.baseline_tree.as_deref()).await?,
            );
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
        // baseline-tree capture was attempted. `tree` may still be `None` when
        // `write-tree` failed, in which case the diff falls back to the index.
        let tree = self.baseline_tree.clone();

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
                let curr_diff = git::get_tree_diff(self.path, tree.as_deref()).await?;
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
                let curr_diff = git::get_tree_diff(self.path, tree.as_deref()).await?;
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
