use std::borrow::Cow;
use std::collections::VecDeque;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex};
use std::time::Duration;

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
    /// Main hook progress line.
    progress: ProgressBar,
    /// Live output preview lines below `progress`.
    output_bars: Vec<ProgressBar>,
    /// Rolling text state rendered into `output_bars`.
    output_preview: OutputPreview,
    /// Result is filled by `on_run_result`; it stays `None` between completion
    /// and result reporting.
    passed: Option<bool>,
}

impl HookBar {
    fn new(hook: &Hook, progress: ProgressBar) -> Self {
        Self {
            hook_key: HookKey::from_hook(hook),
            progress,
            output_bars: Vec::new(),
            output_preview: OutputPreview::default(),
            passed: None,
        }
    }

    fn line_count(&self) -> usize {
        1 + self.output_bars.len()
    }

    /// Streams one output chunk into the preview rows.
    ///
    /// Returns the new visual tail when this chunk caused new preview rows to
    /// be inserted. The owning `HookGroup` uses that tail as the insertion
    /// anchor for subsequent hooks in the same project.
    fn push_output(
        &mut self,
        reporter: &ProgressReporter,
        width: usize,
        chunk: &[u8],
    ) -> Option<ProgressBar> {
        self.output_preview.push_chunk(chunk);
        let lines = self.output_preview.visible_lines();
        let mut inserted_tail = None;

        for (idx, line) in lines.iter().enumerate() {
            if idx == self.output_bars.len() {
                let tail = self.output_bars.last().unwrap_or(&self.progress).clone();
                let output = reporter.children.insert_after(
                    &tail,
                    ProgressBar::with_draw_target(None, reporter.printer.target()),
                );
                output.set_style(
                    ProgressStyle::with_template("{prefix:.dim}{wide_msg:.dim}").unwrap(),
                );
                output.set_prefix(HOOK_OUTPUT_PREVIEW_PREFIX);
                self.output_bars.push(output);
                inserted_tail = self.output_bars.last().cloned();
            }

            let line = line.trim_end();
            let message = if width == 0 {
                String::new()
            } else {
                truncate_to_width(line, width).into_owned()
            };
            self.output_bars[idx].set_message(message);
        }

        inserted_tail
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
    visible: VecDeque<HookBar>,
    hidden_passed: usize,
    hidden_failed: usize,
}

impl CompletedBars {
    fn push(&mut self, completed: HookBar) {
        self.visible.push_back(completed);
    }

    fn hide_one_line(&mut self) -> VecDeque<HookBar> {
        let count = self.hide_count();
        debug_assert!(self.can_hide_one_line());

        let removed: VecDeque<_> = self.visible.drain(..count).collect();
        for completed in &removed {
            match completed.passed {
                Some(true) => self.hidden_passed += 1,
                Some(false) => self.hidden_failed += 1,
                None => {}
            }
        }

        removed
    }

    fn record_result(&mut self, hook_key: HookKey, passed: bool) -> Option<ProgressBar> {
        if let Some(completed) = self
            .visible
            .iter_mut()
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

    fn can_hide_one_line(&self) -> bool {
        let count = self.hide_count();
        self.visible.len() >= count
            && self
                .visible
                .iter()
                .take(count)
                .all(|completed| completed.passed.is_some())
    }

    fn hidden_count(&self) -> usize {
        self.hidden_passed + self.hidden_failed
    }

    fn hide_count(&self) -> usize {
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

    fn clear(&mut self) -> VecDeque<HookBar> {
        self.hidden_passed = 0;
        self.hidden_failed = 0;
        std::mem::take(&mut self.visible)
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
    /// Current insertion anchor for the next line in this project.
    last_line: Option<ProgressBar>,
    /// Running hook whose preview rows currently extend `last_line`.
    active_tail: Option<usize>,
    /// Summary line for completed hooks hidden to fit the terminal height.
    hidden_summary: Option<ProgressBar>,
    /// Completed hook rows owned by this project.
    completed: CompletedBars,
}

impl HookGroup {
    fn new(order: usize, header: Option<ProgressBar>) -> Self {
        let last_line = header.clone();
        Self {
            order,
            header,
            last_line,
            active_tail: None,
            hidden_summary: None,
            completed: CompletedBars::default(),
        }
    }

    fn line_count(&self) -> usize {
        usize::from(self.header.is_some()) + self.completed.line_count()
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
/// layout state and completed hook rows. Keeping those maps separate lets output
/// updates touch one running hook first and update the project insertion anchor
/// only when the hook grows new preview rows.
pub(crate) struct HookRunReporter {
    reporter: Arc<ProgressReporter>,
    dots: usize,
    show_project_headers: bool,
    /// Active hooks keyed by the id returned from `on_run_start`.
    running: Mutex<FxHashMap<usize, HookBar>>,
    /// Per-project layout and completed-hook state.
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
        for completed in
            self.collapse_to_fit_new_progress(&mut groups, Self::running_lines(&running), 1)
        {
            self.remove_hook_bar(completed);
        }
        let group = groups.get_mut(&project_idx).unwrap();
        let progress = self.hook_progress_bar(group.last_line.as_ref(), hook, progress_len);
        group.last_line = Some(progress.clone());
        group.active_tail = Some(id);

        running.insert(id, HookBar::new(hook, progress));
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
        let update = {
            let mut running = self.running.lock().unwrap();
            let update = {
                let Some(run_bar) = running.get_mut(&id) else {
                    return;
                };

                run_bar
                    .push_output(&self.reporter, width, chunk)
                    .map(|tail| (run_bar.hook_key.project_idx, tail))
            };

            update.map(|(project_idx, tail)| (project_idx, tail, Self::running_lines(&running)))
        };
        let Some((project_idx, tail, running_lines)) = update else {
            return;
        };

        let mut groups = self.groups.lock().unwrap();
        if let Some(group) = groups.get_mut(&project_idx)
            && group.active_tail == Some(id)
        {
            // New hooks in this project should be inserted after the active hook's preview.
            group.last_line = Some(tail);
        }

        // Growing preview rows may exceed the terminal height; hide old completed rows.
        let removed = self.collapse_to_fit_new_progress(&mut groups, running_lines, 0);
        drop(groups);
        for completed in removed {
            self.remove_hook_bar(completed);
        }
    }

    pub fn on_run_complete(&self, id: usize) {
        let mut completed = {
            let mut running = self.running.lock().unwrap();
            running.remove(&id).unwrap()
        };
        self.reporter.root.inc(1);

        for output_bar in &completed.output_bars {
            self.reporter.children.remove(output_bar);
        }
        completed.output_bars.clear();

        // Keep the completed line visible until the group result is rendered.
        let progress = &completed.progress;
        progress.set_position(progress.length().unwrap_or(1));
        progress.finish();
        self.remember_completed(id, completed);
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

    fn remember_completed(&self, id: usize, completed: HookBar) {
        let mut groups = self.groups.lock().unwrap();
        let Some(group) = groups.get_mut(&completed.hook_key.project_idx) else {
            drop(groups);
            self.remove_hook_bar(completed);
            return;
        };
        if group.active_tail == Some(id) {
            group.last_line = Some(completed.progress.clone());
            group.active_tail = None;
        }
        group.completed.push(completed);
    }

    fn clear_group(&self, mut group: HookGroup) {
        if let Some(header) = group.header {
            self.reporter.children.remove(&header);
        }
        if let Some(summary) = group.hidden_summary {
            self.reporter.children.remove(&summary);
        }
        for completed in group.completed.clear() {
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
            let summary = if let Some(header) = &group.header {
                self.reporter.children.insert_after(
                    header,
                    ProgressBar::with_draw_target(None, self.reporter.printer.target()),
                )
            } else {
                self.reporter.children.insert_before(
                    anchor,
                    ProgressBar::with_draw_target(None, self.reporter.printer.target()),
                )
            };
            summary.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
            group.hidden_summary = Some(summary.clone());
            summary
        };
        if group.completed.visible.is_empty() {
            group.last_line = Some(summary.clone());
        }
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

    fn active_lines(groups: &HookGroups, running: usize) -> usize {
        let group_lines = groups.values().map(HookGroup::line_count).sum::<usize>();
        1 + running + group_lines
    }

    fn collapse_candidate(groups: &HookGroups) -> Option<usize> {
        groups
            .iter()
            .filter(|(_, group)| group.completed.can_hide_one_line())
            .min_by_key(|(_, group)| group.order)
            .map(|(project_idx, _)| *project_idx)
    }

    fn collapse_to_fit_new_progress(
        &self,
        groups: &mut HookGroups,
        running: usize,
        new_lines: usize,
    ) -> Vec<HookBar> {
        let Some(limit) = self.progress_line_limit() else {
            return Vec::new();
        };

        let mut removed = Vec::new();
        while Self::active_lines(groups, running).saturating_add(new_lines) > limit {
            let Some(project_idx) = Self::collapse_candidate(groups) else {
                break;
            };
            let group = groups.get_mut(&project_idx).unwrap();

            let hidden = group.completed.hide_one_line();
            let anchor = hidden.front().unwrap().progress.clone();
            self.update_group_summary(group, &anchor);
            removed.extend(hidden);
        }

        removed
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
            progress: ProgressBar::hidden(),
            output_bars: Vec::new(),
            output_preview: OutputPreview::default(),
            passed,
        }
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
    fn hiding_completed_bars_frees_one_line() {
        let mut completed = CompletedBars::default();

        completed.push(completed_bar(0, Some(true)));
        assert!(!completed.can_hide_one_line());

        completed.push(completed_bar(1, Some(false)));
        completed.push(completed_bar(2, Some(true)));
        let removed = completed.hide_one_line();
        assert_eq!(removed.len(), 2);
        assert_eq!(completed.visible.len(), 1);
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 2 hooks hidden: 1 passed, 1 failed")
        );

        let removed = completed.hide_one_line();
        assert_eq!(removed.len(), 1);
        assert_eq!(completed.visible.len(), 0);
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 3 hooks hidden: 2 passed, 1 failed")
        );
    }

    #[test]
    fn hiding_requires_a_known_result_prefix() {
        let mut completed = CompletedBars::default();

        completed.push(completed_bar(0, None));
        completed.push(completed_bar(1, Some(true)));
        completed.push(completed_bar(2, Some(true)));

        assert!(!completed.can_hide_one_line());
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
    fn active_lines_includes_root_running_and_group_lines() {
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

        assert_eq!(HookRunReporter::active_lines(&groups, 2), 7);
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

        reporter.update_group_summary(&mut group, &ProgressBar::hidden());

        let summary = group.hidden_summary.as_ref().unwrap();
        let message = summary.message().clone();
        assert!(message.starts_with("  "));
        assert!(message.contains("⋮ 3 hooks hidden: 2 passed, 1 failed"));
        assert_eq!(
            group.last_line.as_ref().unwrap().message(),
            summary.message()
        );
    }

    #[test]
    fn update_group_summary_uses_anchor_without_project_header() {
        let reporter = HookRunReporter::new(Printer::Silent, 80, false);
        let anchor = progress_bar(&reporter);
        let mut group = hook_group(0, false);
        group.completed.hidden_failed = 1;

        reporter.update_group_summary(&mut group, &anchor);

        let summary = group.hidden_summary.as_ref().unwrap();
        let message = summary.message().clone();
        assert!(!message.starts_with("  "));
        assert!(message.contains("⋮ 1 hooks hidden: 1 failed"));
        assert_eq!(
            group.last_line.as_ref().unwrap().message(),
            summary.message()
        );
    }

    #[test]
    fn update_group_summary_is_noop_without_hidden_completed() {
        let reporter = HookRunReporter::new(Printer::Silent, 80, false);
        let anchor = progress_bar(&reporter);
        let mut group = hook_group(0, false);

        reporter.update_group_summary(&mut group, &anchor);

        assert!(group.hidden_summary.is_none());
        assert!(group.last_line.is_none());
    }
}
