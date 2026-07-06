//! Progress UI for concurrent hook execution.
//!
//! This module renders one root progress line, optional project header lines,
//! hook progress rows, short live output previews, and collapsed summaries for
//! completed hooks that no longer fit in the terminal.
//!
//! # Row model
//!
//! A hook run is represented by a `HookBar`. Its main progress row is always
//! present while the hook is active or waiting for its final result. If the hook
//! streams output for long enough, up to `HOOK_OUTPUT_PREVIEW_LINES` preview
//! rows are inserted directly below the main row.
//!
//! `HookRunReporter::running` owns active `HookBar`s. `HookGroup` owns the
//! per-project rows that can outlive an active hook: the optional project
//! header, the collapsed hidden-summary row, and completed hook rows. A
//! completed hook moves from `running` into its project's `CompletedBars` before
//! the running lock is released, so other layout operations never observe the
//! hook as missing from both states.
//!
//! # Visual order and anchors
//!
//! Each hook main row receives a monotonic `line_order` when it starts. Hooks
//! may complete in a different order, so completed rows are stored by
//! `line_order`, not by completion time or hook index.
//!
//! A newly started hook is inserted after the visually latest row in its
//! project. The candidates are the collapsed summary row, the latest running
//! hook's visual tail, and the latest visible completed hook row. If the project
//! has none of those rows, the project header is used; otherwise the hook is
//! inserted before the root progress line. Preview rows do not receive their own
//! `line_order`; they are addressed through the owning hook's `visual_tail`.
//!
//! # Collapse invariants
//!
//! Terminal-height pressure is handled by collapsing old completed hook rows
//! into one summary line. Only completed rows with a known pass/fail result can
//! be collapsed, because the summary has to preserve the result counts. The
//! first collapse hides two completed rows to make room for the new summary; a
//! later collapse hides one more row and reuses the existing summary.
//!
//! The summary row's visual order is fixed at the first hidden completed row.
//! It intentionally does not advance as later completed rows are hidden:
//! running rows can still be visually interleaved with completed rows from the
//! same project, and moving the summary order forward would make future hooks
//! insert above those still-running rows.
//!
//! # Synchronization
//!
//! `running` is the single source of truth for active hook rows and their
//! preview tails. `HookGroup` does not cache running row positions. Code paths
//! that need both maps lock `running` before `groups`, matching the state
//! transition from running to completed and avoiding stale insertion anchors.

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use console::{Term, strip_ansi_codes};
use indicatif::{ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use rustc_hash::FxHashMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::cli::reporter::{ProgressReporter, SPINNER_TICKS, set_current_reporter};
use crate::hook::Hook;
use crate::printer::Printer;
use crate::process::OutputSink;
use crate::workspace;

/// UI state for one hook run.
///
/// A hook occupies one main progress line and, once it emits output, zero to
/// `HOOK_OUTPUT_PREVIEW_LINES` preview lines inserted directly below it.
/// While the hook is running, `HookRunReporter::running` owns this value. After
/// the hook completes, it moves into the owning project's `HookGroup::completed`
/// until the group is cleared or collapsed.
#[derive(Debug)]
struct HookBar {
    /// Stable identity used to match a completed bar with the later hook result.
    hook_key: HookKey,
    /// Monotonic visual insertion order of the main hook progress line.
    line_order: usize,
    /// Main hook progress line.
    progress: ProgressBar,
    /// Live output preview lines below `progress`.
    output_bars: Vec<ProgressBar>,
    /// Rolling text state rendered into `output_bars`.
    output_preview: OutputPreview,
    /// Hook start time, used to avoid flashing output preview rows for fast hooks.
    started_at: Instant,
    /// Result is filled by `on_run_result`; it stays `None` between completion
    /// and result reporting.
    passed: Option<bool>,
}

impl HookBar {
    fn new(hook: &Hook, line_order: usize, progress: ProgressBar) -> Self {
        Self {
            hook_key: HookKey::from_hook(hook),
            line_order,
            progress,
            output_bars: Vec::new(),
            output_preview: OutputPreview::default(),
            started_at: Instant::now(),
            passed: None,
        }
    }

    fn line_count(&self) -> usize {
        1 + self.output_bars.len()
    }

    /// Streams one output chunk into the preview rows.
    ///
    /// Returns whether this chunk inserted any new preview rows.
    fn push_output(&mut self, reporter: &ProgressReporter, width: usize, chunk: &[u8]) -> bool {
        self.output_preview.push_chunk(chunk);
        if self.output_bars.is_empty() && self.started_at.elapsed() < HOOK_OUTPUT_PREVIEW_DELAY {
            return false;
        }

        let lines = self.output_preview.visible_lines();
        let mut inserted = false;

        for (idx, line) in lines.iter().enumerate() {
            if idx == self.output_bars.len() {
                let tail = self.visual_tail();
                let output = reporter.children.insert_after(
                    tail,
                    ProgressBar::with_draw_target(None, reporter.printer.target()),
                );
                output.set_style(
                    ProgressStyle::with_template("{prefix:.dim}{wide_msg:.dim}").unwrap(),
                );
                output.set_prefix(HOOK_OUTPUT_PREVIEW_PREFIX);
                self.output_bars.push(output);
                inserted = true;
            }

            let line = line.trim_end();
            let message = if width == 0 {
                String::new()
            } else {
                truncate_to_width(line, width).into_owned()
            };
            self.output_bars[idx].set_message(message);
        }

        inserted
    }

    fn visual_tail(&self) -> &ProgressBar {
        self.output_bars.last().unwrap_or(&self.progress)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HookKey {
    project_idx: usize,
    hook_idx: usize,
}

impl HookKey {
    fn from_hook(hook: &Hook) -> Self {
        Self {
            project_idx: hook.project().idx(),
            hook_idx: hook.idx,
        }
    }
}

#[derive(Debug, Default)]
struct CompletedBars {
    visible: BTreeMap<usize, HookBar>,
    /// Visual order of the collapsed summary line, fixed at the first hidden row.
    hidden_summary_order: Option<usize>,
    hidden_passed: usize,
    hidden_failed: usize,
}

#[derive(Debug)]
struct CollapsedCompletedBars {
    removed: Vec<HookBar>,
}

impl CollapsedCompletedBars {
    fn anchor(&self) -> &ProgressBar {
        &self.removed[0].progress
    }
}

impl CompletedBars {
    fn push(&mut self, completed: HookBar) {
        // Hooks can finish in a different order than their progress rows were inserted;
        // collapse completed rows by their original visual order.
        let replaced = self.visible.insert(completed.line_order, completed);
        debug_assert!(replaced.is_none());
    }

    fn collapse_one_line(&mut self) -> Option<CollapsedCompletedBars> {
        if !self.can_collapse_one_line() {
            return None;
        }

        let count = self.collapse_count();
        let mut removed = Vec::with_capacity(count);
        for _ in 0..count {
            let (line_order, completed) = self.visible.pop_first()?;
            self.hidden_summary_order.get_or_insert(line_order);
            match completed.passed {
                Some(true) => self.hidden_passed += 1,
                Some(false) => self.hidden_failed += 1,
                None => {}
            }
            removed.push(completed);
        }

        Some(CollapsedCompletedBars { removed })
    }

    fn record_result(&mut self, hook_key: HookKey, passed: bool) -> Option<ProgressBar> {
        if let Some(completed) = self
            .visible
            .values_mut()
            .find(|completed| completed.hook_key == hook_key)
        {
            completed.passed = Some(passed);
            return Some(completed.progress.clone());
        }

        None
    }

    fn line_count(&self) -> usize {
        self.visible.len() + usize::from(self.hidden_count() > 0)
    }

    fn can_collapse_one_line(&self) -> bool {
        let count = self.collapse_count();
        self.visible.len() >= count
            && self
                .visible
                .values()
                .take(count)
                .all(|completed| completed.passed.is_some())
    }

    fn hidden_count(&self) -> usize {
        self.hidden_passed + self.hidden_failed
    }

    fn collapse_count(&self) -> usize {
        // The first collapse must free one row for the summary line. Once the
        // summary exists, hiding one more completed hook frees one visible row.
        if self.hidden_count() > 0 { 1 } else { 2 }
    }

    fn hidden_summary(&self) -> Option<String> {
        let hidden = self.hidden_count();
        if hidden == 0 {
            return None;
        }

        let status = match (self.hidden_passed, self.hidden_failed) {
            (passed, 0) => format!("{passed} passed"),
            (0, failed) => format!("{failed} failed"),
            (passed, failed) => format!("{passed} passed, {failed} failed"),
        };
        Some(format!("⋮ {hidden} hooks hidden: {status}"))
    }

    fn last_visible(&self) -> Option<(usize, &ProgressBar)> {
        self.visible
            .last_key_value()
            .map(|(line_order, completed)| (*line_order, &completed.progress))
    }

    fn drain_visible(&mut self) -> impl Iterator<Item = HookBar> {
        self.hidden_summary_order = None;
        self.hidden_passed = 0;
        self.hidden_failed = 0;
        std::mem::take(&mut self.visible).into_values()
    }
}

/// Per-project layout state for hook execution.
///
/// Running hooks are stored globally in `HookRunReporter::running`; this group
/// tracks where that project's next hook should be inserted, which completed
/// hook rows are still visible, and whether a collapsed summary line exists.
#[derive(Debug)]
struct HookGroup {
    /// Project creation order, used to collapse older groups first when the terminal is full.
    order: usize,
    /// Optional project header line shown above hooks when project headers are enabled.
    header: Option<ProgressBar>,
    /// Summary line for completed hooks hidden to fit the terminal height.
    hidden_summary: Option<ProgressBar>,
    /// Completed hook rows owned by this project.
    completed: CompletedBars,
}

impl HookGroup {
    fn new(order: usize, header: Option<ProgressBar>) -> Self {
        Self {
            order,
            header,
            hidden_summary: None,
            completed: CompletedBars::default(),
        }
    }

    fn line_count(&self) -> usize {
        usize::from(self.header.is_some()) + self.completed.line_count()
    }

    fn insertion_anchor<'a>(
        &'a self,
        project_idx: usize,
        running: &'a FxHashMap<usize, HookBar>,
    ) -> Option<&'a ProgressBar> {
        let hidden_summary = self
            .completed
            .hidden_summary_order
            .zip(self.hidden_summary.as_ref());
        let latest_running = running
            .values()
            .filter(|bar| bar.hook_key.project_idx == project_idx)
            .max_by_key(|bar| bar.line_order)
            .map(|bar| (bar.line_order, bar.visual_tail()));
        let latest_completed = self.completed.last_visible();

        hidden_summary
            .into_iter()
            .chain(latest_running)
            .chain(latest_completed)
            .max_by_key(|(line_order, _)| *line_order)
            .map(|(_, progress)| progress)
            .or(self.header.as_ref())
    }
}

/// Project groups keyed by `workspace::Project::idx()`.
type HookGroups = FxHashMap<usize, HookGroup>;

pub(crate) fn project_status_marker(failed: bool) -> String {
    if failed {
        "×".red().to_string()
    } else {
        "✓".green().to_string()
    }
}

/// Rolling text preview for a running hook's streamed output.
///
/// `lines` is always the visible window, capped at `HOOK_OUTPUT_PREVIEW_LINES`.
/// If `line_open` is true, the last line is still accepting characters from the
/// current unterminated output line. A pending carriage return either joins a
/// following `\n` as CRLF or clears that current line to emulate terminal
/// "overwrite this line" output.
#[derive(Debug, Default)]
struct OutputPreview {
    lines: Vec<String>,
    line_open: bool,
    pending_cr: bool,
}

impl OutputPreview {
    fn push_chunk(&mut self, chunk: &[u8]) {
        // Preview text is lossy by design: the full bytes are still collected by `process`.
        let text = String::from_utf8_lossy(chunk);
        let text = strip_ansi_codes(&text);
        for ch in text.chars().filter(|ch| is_preview_char(*ch)) {
            if self.pending_cr {
                if ch == '\n' {
                    self.finish_line();
                    self.pending_cr = false;
                    continue;
                }
                self.current_line_mut().clear();
                self.pending_cr = false;
            }
            match ch {
                '\n' => self.finish_line(),
                '\r' => self.pending_cr = true,
                '\t' => self.current_line_mut().push(' '),
                ch => self.current_line_mut().push(ch),
            }
        }
    }

    fn visible_lines(&self) -> &[String] {
        &self.lines
    }

    fn current_line_mut(&mut self) -> &mut String {
        if !self.line_open {
            self.lines.push(String::new());
            self.line_open = true;
            self.truncate();
        }
        let idx = self.lines.len() - 1;
        &mut self.lines[idx]
    }

    fn finish_line(&mut self) {
        if self.line_open {
            self.line_open = false;
        } else {
            self.lines.push(String::new());
            self.truncate();
        }
    }

    fn truncate(&mut self) {
        if self.lines.len() > HOOK_OUTPUT_PREVIEW_LINES {
            let overflow = self.lines.len() - HOOK_OUTPUT_PREVIEW_LINES;
            self.lines.drain(..overflow);
        }
    }
}

fn is_preview_char(ch: char) -> bool {
    matches!(ch, '\n' | '\r' | '\t') || !ch.is_control()
}

const HOOK_OUTPUT_PREVIEW_LINES: usize = 3;
const HOOK_OUTPUT_PREVIEW_DELAY: Duration = Duration::from_millis(500);
const HOOK_OUTPUT_PREVIEW_PREFIX: &str = "    => ";

fn truncate_to_width(input: &str, width: usize) -> Cow<'_, str> {
    if input.width() <= width {
        return Cow::Borrowed(input);
    }

    if width <= 3 {
        return Cow::Owned(".".repeat(width));
    }

    let mut output = String::new();
    let mut used = 0;
    let target = width - 3;
    for ch in input.chars() {
        let ch_width = ch.width().unwrap_or(0);
        if used + ch_width > target {
            break;
        }
        output.push(ch);
        used += ch_width;
    }
    output.push_str("...");
    Cow::Owned(output)
}

/// Coordinates the hook-run progress UI.
///
/// `running` owns active hook bars by progress id. `groups` owns per-project
/// layout state and completed hook rows. The insertion anchor for a new hook is
/// derived from both maps while `running` is locked, so there is no separate
/// per-project cache of running row positions to keep in sync.
pub(crate) struct HookRunReporter {
    reporter: Arc<ProgressReporter>,
    dots: usize,
    show_project_headers: bool,
    /// Active hooks keyed by the id returned from `on_run_start`.
    running: Mutex<FxHashMap<usize, HookBar>>,
    /// Per-project layout and completed-hook state.
    ///
    /// Code paths that move rows between running and completed state lock
    /// `running` before `groups`, so `on_run_start` cannot observe a hook as
    /// neither running nor completed.
    groups: Mutex<HookGroups>,
}

impl HookRunReporter {
    pub fn new(printer: Printer, dots: usize, show_project_headers: bool) -> Self {
        let reporter = Arc::new(ProgressReporter::from(printer));
        reporter.set_root_prefix("Running hooks...");
        set_current_reporter(Some(&reporter));

        Self {
            reporter,
            dots,
            show_project_headers,
            running: Mutex::default(),
            groups: Mutex::default(),
        }
    }

    pub fn on_run_start(&self, hook: &Hook, len: usize) -> usize {
        let id = self.reporter.next_id();
        let progress_len = if len == 0 { 1 } else { len as u64 };

        let mut running = self.running.lock().unwrap();
        let mut groups = self.groups.lock().unwrap();
        let project_idx = hook.project().idx();
        let order = groups.len();
        if let Entry::Vacant(entry) = groups.entry(project_idx) {
            entry.insert(HookGroup::new(order, self.project_header(hook)));
        }
        let group = groups.get_mut(&project_idx).unwrap();
        let progress = self.hook_progress_bar(
            group.insertion_anchor(project_idx, &running),
            hook,
            progress_len,
        );

        running.insert(id, HookBar::new(hook, id, progress));
        self.ensure_progress_capacity(&mut groups, &running);
        id
    }

    fn project_header(&self, hook: &Hook) -> Option<ProgressBar> {
        if !self.show_project_headers {
            return None;
        }

        let header = self.reporter.children.insert_before(
            &self.reporter.root,
            ProgressBar::with_draw_target(None, self.reporter.printer.target()),
        );
        header.enable_steady_tick(Duration::from_millis(200));
        header.set_style(
            ProgressStyle::with_template("{spinner:.cyan} {wide_msg}")
                .unwrap()
                .tick_strings(SPINNER_TICKS),
        );
        header.set_message(format!("{}", hook.project().display_name().cyan().bold()));
        Some(header)
    }

    fn hook_progress_bar(
        &self,
        anchor: Option<&ProgressBar>,
        hook: &Hook,
        progress_len: u64,
    ) -> ProgressBar {
        let progress = match anchor {
            Some(anchor) => self.reporter.children.insert_after(
                anchor,
                ProgressBar::with_draw_target(Some(progress_len), self.reporter.printer.target()),
            ),
            None => self.reporter.children.insert_before(
                &self.reporter.root,
                ProgressBar::with_draw_target(Some(progress_len), self.reporter.printer.target()),
            ),
        };

        let label = if self.show_project_headers {
            format!("  {}", hook.name)
        } else {
            hook.name.clone()
        };
        let dots = self.dots.saturating_sub(label.width());
        progress.set_style(
            ProgressStyle::with_template(&format!("{{msg}}{{bar:{dots}.green/dim}}"))
                .unwrap()
                .progress_chars(".."),
        );
        progress.set_message(label);
        progress
    }

    pub fn on_run_progress(&self, id: usize, completed: u64) {
        let running = self.running.lock().unwrap();
        let progress = &running[&id].progress;
        progress.inc(completed);
    }

    pub(crate) fn output_sink(&self, id: usize) -> HookOutputSink<'_> {
        HookOutputSink {
            reporter: self,
            progress: id,
        }
    }

    fn on_run_output(&self, id: usize, chunk: &[u8]) {
        let width = self.dots.saturating_sub(HOOK_OUTPUT_PREVIEW_PREFIX.width());
        let mut running = self.running.lock().unwrap();
        let Some(run_bar) = running.get_mut(&id) else {
            return;
        };
        if !run_bar.push_output(&self.reporter, width, chunk) {
            return;
        }

        let mut groups = self.groups.lock().unwrap();
        self.ensure_progress_capacity(&mut groups, &running);
    }

    pub fn on_run_complete(&self, id: usize) {
        enum CompletedPlacement {
            Stored(Vec<ProgressBar>),
            Orphan(HookBar),
        }

        let placement = {
            let mut running = self.running.lock().unwrap();
            let mut completed = running.remove(&id).unwrap();
            self.reporter.root.inc(1);

            // Keep the completed line visible until the group result is rendered.
            let progress = &completed.progress;
            progress.set_position(progress.length().unwrap_or(1));
            progress.finish();

            // Move the hook into its group before releasing `running`, so layout
            // accounting never observes the main row as neither running nor completed.
            let mut groups = self.groups.lock().unwrap();
            if let Some(group) = groups.get_mut(&completed.hook_key.project_idx) {
                let output_bars = std::mem::take(&mut completed.output_bars);
                group.completed.push(completed);
                CompletedPlacement::Stored(output_bars)
            } else {
                CompletedPlacement::Orphan(completed)
            }
        };

        match placement {
            CompletedPlacement::Stored(output_bars) => {
                for output_bar in output_bars {
                    self.reporter.children.remove(&output_bar);
                }
            }
            CompletedPlacement::Orphan(completed) => self.remove_hook_bar(completed),
        }
    }

    pub fn on_run_result(&self, hook: &Hook, passed: bool) {
        let hook_key = HookKey::from_hook(hook);
        let progress = {
            let mut groups = self.groups.lock().unwrap();
            let Some(group) = groups.get_mut(&hook_key.project_idx) else {
                return;
            };
            group.completed.record_result(hook_key, passed)
        };
        let Some(progress) = progress else {
            return;
        };

        let label = progress.message();
        let (status, status_width) = if passed {
            ("Passed".on_green().to_string(), "Passed".width())
        } else {
            ("Failed".on_red().to_string(), "Failed".width())
        };
        let dots = self
            .dots
            .saturating_add("Passed".width())
            .saturating_sub(label.width() + status_width);
        let dots = ".".repeat(dots).green().to_string();

        progress.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
        progress.set_message(format!("{label}{dots}{status}"));
        progress.finish();
    }

    pub fn on_project_complete(&self, project: &workspace::Project, failed: bool) {
        let mut groups = self.groups.lock().unwrap();
        let Some(group) = groups.get_mut(&project.idx()) else {
            return;
        };
        let Some(header) = &group.header else {
            return;
        };
        header.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
        header.set_message(format!(
            "{} {}",
            project_status_marker(failed),
            project.display_name().cyan().bold()
        ));

        header.finish();
    }

    pub fn clear_completed(&self) {
        let groups = {
            let mut groups = self.groups.lock().unwrap();
            std::mem::take(&mut *groups)
        };

        for (_, group) in groups {
            self.clear_group(group);
        }
    }

    /// Temporarily suspend progress rendering while emitting normal output.
    ///
    /// This helps prevent the progress UI from being corrupted by concurrent writes.
    pub fn suspend<R>(&self, f: impl FnOnce() -> R) -> R {
        self.reporter.children.suspend(f)
    }

    pub fn on_complete(&self) {
        self.clear_completed();
        self.reporter.on_complete();
    }

    fn clear_group(&self, mut group: HookGroup) {
        if let Some(header) = group.header {
            self.reporter.children.remove(&header);
        }
        if let Some(summary) = group.hidden_summary {
            self.reporter.children.remove(&summary);
        }
        for completed in group.completed.drain_visible() {
            self.remove_hook_bar(completed);
        }
    }

    fn remove_hook_bar(&self, hook_bar: HookBar) {
        self.reporter.children.remove(&hook_bar.progress);
        for output_bar in hook_bar.output_bars {
            self.reporter.children.remove(&output_bar);
        }
    }

    fn running_lines(running: &FxHashMap<usize, HookBar>) -> usize {
        running.values().map(HookBar::line_count).sum()
    }

    fn update_group_summary(&self, group: &mut HookGroup, anchor: &ProgressBar) {
        let Some(message) = group.completed.hidden_summary() else {
            return;
        };

        let summary = if let Some(summary) = &group.hidden_summary {
            summary.clone()
        } else {
            let summary = self.reporter.children.insert_before(
                anchor,
                ProgressBar::with_draw_target(None, self.reporter.printer.target()),
            );
            summary.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
            group.hidden_summary = Some(summary.clone());
            summary
        };
        if group.header.is_some() {
            summary.set_message(format!("  {}", message.dimmed()));
        } else {
            summary.set_message(format!("{}", message.dimmed()));
        }
    }

    fn progress_line_limit(&self) -> Option<usize> {
        if self.reporter.children.is_hidden() {
            return None;
        }

        Term::stderr()
            .size_checked()
            .map(|(height, _)| usize::from(height))
            .filter(|height| *height > 0)
    }

    fn progress_line_count(groups: &HookGroups, running_lines: usize) -> usize {
        let group_lines = groups.values().map(HookGroup::line_count).sum::<usize>();
        1 + running_lines + group_lines
    }

    fn collapse_candidate(groups: &HookGroups) -> Option<usize> {
        groups
            .iter()
            .filter(|(_, group)| group.completed.can_collapse_one_line())
            .min_by_key(|(_, group)| group.order)
            .map(|(project_idx, _)| *project_idx)
    }

    fn ensure_progress_capacity(
        &self,
        groups: &mut HookGroups,
        running: &FxHashMap<usize, HookBar>,
    ) {
        let Some(limit) = self.progress_line_limit() else {
            return;
        };

        let running_lines = Self::running_lines(running);
        while Self::progress_line_count(groups, running_lines) > limit {
            let Some(project_idx) = Self::collapse_candidate(groups) else {
                break;
            };

            let collapsed = {
                let group = groups.get_mut(&project_idx).unwrap();
                let Some(collapsed) = group.completed.collapse_one_line() else {
                    break;
                };
                self.update_group_summary(group, collapsed.anchor());
                collapsed
            };
            for completed in collapsed.removed {
                self.remove_hook_bar(completed);
            }
        }
    }
}

pub(crate) struct HookOutputSink<'a> {
    reporter: &'a HookRunReporter,
    progress: usize,
}

impl OutputSink for HookOutputSink<'_> {
    fn write_chunk(&mut self, chunk: &[u8]) {
        self.reporter.on_run_output(self.progress, chunk);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn completed_bar(hook_idx: usize, passed: Option<bool>) -> HookBar {
        project_completed_bar(0, hook_idx, passed)
    }

    fn project_completed_bar(project_idx: usize, hook_idx: usize, passed: Option<bool>) -> HookBar {
        HookBar {
            hook_key: HookKey {
                project_idx,
                hook_idx,
            },
            line_order: hook_idx,
            progress: ProgressBar::hidden(),
            output_bars: Vec::new(),
            output_preview: OutputPreview::default(),
            started_at: Instant::now(),
            passed,
        }
    }

    fn running_hook_bar(reporter: &HookRunReporter, started_at: Instant) -> HookBar {
        HookBar {
            hook_key: HookKey {
                project_idx: 0,
                hook_idx: 0,
            },
            line_order: 0,
            progress: progress_bar(reporter),
            output_bars: Vec::new(),
            output_preview: OutputPreview::default(),
            started_at,
            passed: None,
        }
    }

    fn elapsed_start() -> Instant {
        Instant::now()
            .checked_sub(HOOK_OUTPUT_PREVIEW_DELAY + Duration::from_millis(1))
            .unwrap()
    }

    fn hook_group(order: usize, has_header: bool) -> HookGroup {
        let header = if has_header {
            Some(ProgressBar::hidden())
        } else {
            None
        };
        HookGroup::new(order, header)
    }

    fn progress_bar(reporter: &HookRunReporter) -> ProgressBar {
        reporter.reporter.children.insert_before(
            &reporter.reporter.root,
            ProgressBar::with_draw_target(None, reporter.reporter.printer.target()),
        )
    }

    fn hidden_bar(message: &'static str) -> ProgressBar {
        let progress = ProgressBar::hidden();
        progress.set_message(message);
        progress
    }

    fn project_running_bar(project_idx: usize, hook_idx: usize, message: &'static str) -> HookBar {
        let mut bar = project_completed_bar(project_idx, hook_idx, None);
        bar.progress = hidden_bar(message);
        bar
    }

    fn visible_hook_indices(completed: &CompletedBars) -> Vec<usize> {
        completed
            .visible
            .values()
            .map(|bar| bar.hook_key.hook_idx)
            .collect()
    }

    #[test]
    fn hidden_summary_shows_total_and_result_breakdown() {
        let completed = CompletedBars {
            hidden_passed: 8,
            ..CompletedBars::default()
        };
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 8 hooks hidden: 8 passed")
        );

        let completed = CompletedBars {
            hidden_failed: 8,
            ..CompletedBars::default()
        };
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 8 hooks hidden: 8 failed")
        );

        let completed = CompletedBars {
            hidden_passed: 6,
            hidden_failed: 2,
            ..CompletedBars::default()
        };
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 8 hooks hidden: 6 passed, 2 failed")
        );
    }

    #[test]
    fn output_preview_keeps_crlf_line() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"processing file\r\n");

        assert_eq!(preview.visible_lines(), ["processing file"]);
    }

    #[test]
    fn output_preview_handles_split_crlf() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"processing file\r");
        preview.push_chunk(b"\n");

        assert_eq!(preview.visible_lines(), ["processing file"]);
    }

    #[test]
    fn output_preview_replaces_carriage_return_line() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"first\rsecond");

        assert_eq!(preview.visible_lines(), ["second"]);
    }

    #[test]
    fn output_preview_strips_ansi_codes() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"\x1b[31mred\x1b[0m\n");

        assert_eq!(preview.visible_lines(), ["red"]);
    }

    #[test]
    fn output_preview_keeps_last_preview_window() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"one\ntwo\nthree\nfour\n");

        assert_eq!(preview.visible_lines(), ["two", "three", "four"]);
    }

    #[test]
    fn hook_output_preview_is_buffered_before_delay() {
        let reporter = HookRunReporter::new(Printer::Silent, 80, false);
        let mut hook_bar = running_hook_bar(&reporter, Instant::now());

        let inserted = hook_bar.push_output(&reporter.reporter, 80, b"first\n");

        assert!(!inserted);
        assert!(hook_bar.output_bars.is_empty());
        assert_eq!(hook_bar.output_preview.visible_lines(), ["first"]);
    }

    #[test]
    fn hook_output_preview_shows_buffered_lines_after_delay() {
        let reporter = HookRunReporter::new(Printer::Silent, 80, false);
        let mut hook_bar = running_hook_bar(&reporter, Instant::now());

        hook_bar.push_output(&reporter.reporter, 80, b"first\n");
        hook_bar.started_at = elapsed_start();
        let inserted = hook_bar.push_output(&reporter.reporter, 80, b"second\n");

        assert!(inserted);
        let messages = hook_bar
            .output_bars
            .iter()
            .map(|bar| bar.message().clone())
            .collect::<Vec<_>>();
        assert_eq!(messages, ["first", "second"]);
    }

    #[test]
    fn collapsing_completed_bars_frees_one_line() {
        let mut completed = CompletedBars::default();

        completed.push(completed_bar(0, Some(true)));
        assert!(!completed.can_collapse_one_line());

        completed.push(completed_bar(1, Some(false)));
        completed.push(completed_bar(2, Some(true)));
        let collapsed = completed.collapse_one_line().unwrap();
        assert_eq!(collapsed.removed.len(), 2);
        assert_eq!(visible_hook_indices(&completed).len(), 1);
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 2 hooks hidden: 1 passed, 1 failed")
        );

        let collapsed = completed.collapse_one_line().unwrap();
        assert_eq!(collapsed.removed.len(), 1);
        assert!(visible_hook_indices(&completed).is_empty());
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 3 hooks hidden: 2 passed, 1 failed")
        );
    }

    #[test]
    fn collapsing_completed_bars_uses_visual_order_after_out_of_order_completion() {
        let mut completed = CompletedBars::default();

        completed.push(project_completed_bar(0, 4, Some(true)));
        completed.push(project_completed_bar(0, 0, Some(true)));
        completed.push(project_completed_bar(0, 1, Some(true)));
        completed.push(project_completed_bar(0, 2, Some(true)));
        completed.push(project_completed_bar(0, 3, Some(true)));

        let collapsed = completed.collapse_one_line().unwrap();
        let removed_hooks = collapsed
            .removed
            .iter()
            .map(|bar| bar.hook_key.hook_idx)
            .collect::<Vec<_>>();
        assert_eq!(removed_hooks, [0, 1]);
        assert_eq!(visible_hook_indices(&completed), [2, 3, 4]);
    }

    #[test]
    fn collapsing_completed_bars_keeps_summary_at_first_hidden_row() {
        let mut group = hook_group(0, false);

        group
            .completed
            .push(project_completed_bar(0, 0, Some(true)));
        group
            .completed
            .push(project_completed_bar(0, 2, Some(true)));
        let collapsed = group.completed.collapse_one_line().unwrap();
        let removed_hooks = collapsed
            .removed
            .iter()
            .map(|bar| bar.hook_key.hook_idx)
            .collect::<Vec<_>>();
        group.hidden_summary = Some(hidden_bar("summary"));
        let mut running = FxHashMap::default();
        running.insert(1, project_running_bar(0, 1, "running-1"));

        assert_eq!(removed_hooks, [0, 2]);
        assert_eq!(group.completed.hidden_summary_order, Some(0));
        assert_eq!(
            group.insertion_anchor(0, &running).unwrap().message(),
            "running-1"
        );
    }

    #[test]
    fn collapsing_requires_a_known_result_prefix() {
        let mut completed = CompletedBars::default();

        completed.push(completed_bar(0, None));
        completed.push(completed_bar(1, Some(true)));
        completed.push(completed_bar(2, Some(true)));

        assert!(!completed.can_collapse_one_line());
    }

    #[test]
    fn group_line_count_includes_header_visible_and_hidden_summary() {
        let mut group = hook_group(0, false);
        assert_eq!(group.line_count(), 0);

        group.completed.push(completed_bar(0, Some(true)));
        group.completed.push(completed_bar(1, None));
        assert_eq!(group.line_count(), 2);

        let mut group = hook_group(0, true);
        group.completed.push(completed_bar(0, Some(true)));
        group.completed.hidden_failed = 1;

        assert_eq!(group.line_count(), 3);
    }

    #[test]
    fn progress_line_count_includes_root_running_and_group_lines() {
        let mut groups = HookGroups::default();

        let mut first = hook_group(0, true);
        first
            .completed
            .push(project_completed_bar(1, 0, Some(true)));
        first.completed.hidden_passed = 2;
        groups.insert(1, first);

        let mut second = hook_group(1, false);
        second
            .completed
            .push(project_completed_bar(2, 0, Some(false)));
        groups.insert(2, second);

        assert_eq!(HookRunReporter::progress_line_count(&groups, 2), 7);
    }

    #[test]
    fn collapse_candidate_picks_oldest_hideable_group() {
        let mut groups = HookGroups::default();

        let mut oldest = hook_group(0, false);
        oldest
            .completed
            .push(project_completed_bar(10, 0, Some(true)));
        groups.insert(10, oldest);

        let mut older_hideable = hook_group(1, false);
        older_hideable
            .completed
            .push(project_completed_bar(20, 0, Some(true)));
        older_hideable
            .completed
            .push(project_completed_bar(20, 1, Some(false)));
        groups.insert(20, older_hideable);

        let mut newer_hideable = hook_group(2, false);
        newer_hideable.completed.hidden_passed = 1;
        newer_hideable
            .completed
            .push(project_completed_bar(30, 0, Some(true)));
        groups.insert(30, newer_hideable);

        assert_eq!(HookRunReporter::collapse_candidate(&groups), Some(20));
    }

    #[test]
    fn update_group_summary_creates_project_summary_line() {
        let reporter = HookRunReporter::new(Printer::Silent, 80, true);
        let mut group = HookGroup::new(0, Some(progress_bar(&reporter)));
        group.completed.hidden_passed = 2;
        group.completed.hidden_failed = 1;
        let anchor = progress_bar(&reporter);

        group.completed.hidden_summary_order = Some(2);

        reporter.update_group_summary(&mut group, &anchor);

        let summary = group.hidden_summary.as_ref().unwrap();
        let message = summary.message().clone();
        assert!(message.starts_with("  "));
        assert!(message.contains("⋮ 3 hooks hidden: 2 passed, 1 failed"));
        let running = FxHashMap::default();
        assert_eq!(
            group.insertion_anchor(0, &running).unwrap().message(),
            summary.message()
        );
    }

    #[test]
    fn update_group_summary_uses_anchor_without_project_header() {
        let reporter = HookRunReporter::new(Printer::Silent, 80, false);
        let anchor = progress_bar(&reporter);
        let mut group = hook_group(0, false);
        group.completed.hidden_failed = 1;

        group.completed.hidden_summary_order = Some(0);

        reporter.update_group_summary(&mut group, &anchor);

        let summary = group.hidden_summary.as_ref().unwrap();
        let message = summary.message().clone();
        assert!(!message.starts_with("  "));
        assert!(message.contains("⋮ 1 hooks hidden: 1 failed"));
        let running = FxHashMap::default();
        assert_eq!(
            group.insertion_anchor(0, &running).unwrap().message(),
            summary.message()
        );
    }

    #[test]
    fn insertion_anchor_prefers_running_tail_over_hidden_summary() {
        let mut group = hook_group(0, false);
        group.completed.hidden_summary_order = Some(0);
        group.hidden_summary = Some(hidden_bar("summary"));
        let mut running = FxHashMap::default();
        running.insert(1, project_running_bar(0, 1, "running"));

        assert_eq!(
            group.insertion_anchor(0, &running).unwrap().message(),
            "running"
        );
    }

    #[test]
    fn insertion_anchor_uses_hidden_summary_when_it_is_visual_tail() {
        let mut group = hook_group(0, false);
        group.completed.hidden_summary_order = Some(2);
        group.hidden_summary = Some(hidden_bar("summary"));
        let mut running = FxHashMap::default();
        running.insert(0, project_running_bar(0, 0, "running-0"));

        assert_eq!(
            group.insertion_anchor(0, &running).unwrap().message(),
            "summary"
        );
    }

    #[test]
    fn insertion_anchor_uses_latest_visible_line_order() {
        let mut group = hook_group(0, false);
        let mut running = FxHashMap::default();
        running.insert(1, project_running_bar(0, 1, "running-1"));
        let completed = project_completed_bar(0, 2, Some(true));
        completed.progress.set_message("completed-2");
        group.completed.push(completed);

        assert_eq!(
            group.insertion_anchor(0, &running).unwrap().message(),
            "completed-2"
        );

        running.insert(3, project_running_bar(0, 3, "running-3"));

        assert_eq!(
            group.insertion_anchor(0, &running).unwrap().message(),
            "running-3"
        );
    }

    #[test]
    fn insertion_anchor_uses_running_output_tail() {
        let group = hook_group(0, false);
        let mut running_bar = project_running_bar(0, 1, "running-main");
        running_bar.output_bars.push(hidden_bar("running-tail"));
        let mut running = FxHashMap::default();
        running.insert(1, running_bar);

        assert_eq!(
            group.insertion_anchor(0, &running).unwrap().message(),
            "running-tail"
        );
    }

    #[test]
    fn update_group_summary_is_noop_without_hidden_completed() {
        let reporter = HookRunReporter::new(Printer::Silent, 80, false);
        let anchor = progress_bar(&reporter);
        let mut group = hook_group(0, false);

        reporter.update_group_summary(&mut group, &anchor);

        assert!(group.hidden_summary.is_none());
        let running = FxHashMap::default();
        assert!(group.insertion_anchor(0, &running).is_none());
    }
}
