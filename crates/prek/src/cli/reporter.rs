use std::borrow::Cow;
use std::collections::VecDeque;
use std::collections::hash_map::Entry;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use console::{Term, strip_ansi_codes};
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use rustc_hash::FxHashMap;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::hook::Hook;
use crate::printer::Printer;
use crate::process::OutputSink;
use crate::workspace;

/// Current progress reporter used to suspend rendering while printing normal output.
static CURRENT_REPORTER: Mutex<Option<Weak<ProgressReporter>>> = Mutex::new(None);
const SPINNER_TICKS: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

/// Set the current reporter for lock acquisition warnings.
fn set_current_reporter(reporter: Option<&Arc<ProgressReporter>>) {
    *CURRENT_REPORTER.lock().unwrap() = reporter.map(Arc::downgrade);
}

/// Suspend progress rendering while emitting normal output.
///
/// If a progress reporter is currently active, this runs `f` inside
/// `indicatif::MultiProgress::suspend` to avoid corrupting the progress display.
/// If no reporter is active (or it has already been dropped), this just runs `f`.
pub(crate) fn suspend(f: impl FnOnce() + Send + 'static) {
    let reporter = CURRENT_REPORTER.lock().unwrap().clone();
    match reporter.and_then(|r| r.upgrade()) {
        Some(reporter) => reporter.children.suspend(f),
        None => f(),
    }
}

#[derive(Default, Debug)]
struct BarState {
    /// A map of progress bars, by ID.
    bars: FxHashMap<usize, ProgressBar>,
    /// A monotonic counter for bar IDs.
    id: usize,
}

impl BarState {
    /// Returns a unique ID for a new progress bar.
    fn id(&mut self) -> usize {
        self.id += 1;
        self.id
    }
}

#[derive(Debug)]
struct HookBar {
    hook_key: HookKey,
    progress: ProgressBar,
    output_bars: Vec<ProgressBar>,
    output_preview: OutputPreview,
    passed: Option<bool>,
}

impl HookBar {
    fn line_count(&self) -> usize {
        1 + self.output_bars.len()
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
        let count = if self.is_collapsed() { 1 } else { 2 };
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

    fn visible_len(&self) -> usize {
        self.visible.len()
    }

    fn can_hide_one_line(&self) -> bool {
        let count = if self.is_collapsed() { 1 } else { 2 };
        self.visible
            .iter()
            .take_while(|completed| completed.passed.is_some())
            .count()
            >= count
    }

    fn hidden_len(&self) -> usize {
        self.hidden_passed + self.hidden_failed
    }

    fn is_collapsed(&self) -> bool {
        self.hidden_len() > 0
    }

    fn hidden_summary(&self) -> Option<String> {
        let hidden = self.hidden_len();
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

#[derive(Debug)]
struct HookGroup {
    order: usize,
    header: Option<ProgressBar>,
    last_line: Option<ProgressBar>,
    active_tail: Option<usize>,
    hidden_summary: Option<ProgressBar>,
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
        usize::from(self.header.is_some())
            + self.completed.visible_len()
            + usize::from(self.completed.is_collapsed())
    }
}

type HookGroups = FxHashMap<usize, HookGroup>;

pub(crate) fn project_status_marker(failed: bool) -> String {
    if failed {
        "×".red().to_string()
    } else {
        "✓".green().to_string()
    }
}

#[derive(Debug, Default)]
struct OutputPreview {
    current: String,
    completed: Vec<String>,
    /// A pending `\r` either joins the following `\n` as CRLF or replaces the current line.
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
                self.current.clear();
                self.pending_cr = false;
            }
            match ch {
                '\n' => self.finish_line(),
                '\r' => self.pending_cr = true,
                '\t' => self.current.push(' '),
                ch => self.current.push(ch),
            }
        }
    }

    fn visible_len(&self) -> usize {
        (self.completed.len() + usize::from(!self.current.is_empty()))
            .min(HOOK_OUTPUT_PREVIEW_LINES)
    }

    fn visible_lines(&self) -> impl Iterator<Item = &str> {
        let has_current = !self.current.is_empty();
        let current = if has_current {
            Some(self.current.as_str())
        } else {
            None
        };
        let completed_start = self
            .completed
            .len()
            .saturating_sub(HOOK_OUTPUT_PREVIEW_LINES - usize::from(has_current));

        self.completed
            .iter()
            .skip(completed_start)
            .map(String::as_str)
            .chain(current)
    }

    fn finish_line(&mut self) {
        self.completed.push(std::mem::take(&mut self.current));
        let overflow = self
            .completed
            .len()
            .saturating_sub(HOOK_OUTPUT_PREVIEW_LINES);
        if overflow > 0 {
            self.completed.drain(..overflow);
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

struct ProgressReporter {
    printer: Printer,
    root: ProgressBar,
    state: Arc<Mutex<BarState>>,
    children: MultiProgress,
}

impl ProgressReporter {
    fn new(root: ProgressBar, children: MultiProgress, printer: Printer) -> Self {
        Self {
            printer,
            root,
            state: Arc::default(),
            children,
        }
    }

    fn on_start(&self, msg: impl Into<Cow<'static, str>>) -> usize {
        let mut state = self.state.lock().unwrap();
        let id = state.id();

        let progress = self.children.insert_before(
            &self.root,
            ProgressBar::with_draw_target(None, self.printer.target()),
        );

        progress.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
        progress.set_message(msg);

        state.bars.insert(id, progress);
        id
    }

    fn on_progress(&self, id: usize) {
        let progress = {
            let mut state = self.state.lock().unwrap();
            state.bars.remove(&id).unwrap()
        };

        self.root.inc(1);
        progress.finish_and_clear();
    }

    fn set_root_prefix(&self, prefix: impl Into<Cow<'static, str>>) {
        self.root.set_prefix(prefix);
    }

    fn on_complete(&self) {
        self.root.set_prefix("");
        self.root.set_message("");
        self.root.finish_and_clear();
    }
}

impl From<Printer> for ProgressReporter {
    fn from(printer: Printer) -> Self {
        let multi = MultiProgress::with_draw_target(printer.target());
        let root = multi.add(ProgressBar::with_draw_target(None, printer.target()));
        root.enable_steady_tick(Duration::from_millis(200));
        root.set_style(
            ProgressStyle::with_template(
                "{spinner:.cyan.bold.dim} {prefix:.cyan.bold.dim}{msg:.dim}",
            )
            .unwrap()
            .tick_strings(SPINNER_TICKS),
        );

        Self::new(root, multi, printer)
    }
}

pub(crate) struct HookInitReporter {
    reporter: Arc<ProgressReporter>,
}

impl HookInitReporter {
    pub(crate) fn new(printer: Printer) -> Self {
        let reporter = Arc::new(ProgressReporter::from(printer));
        set_current_reporter(Some(&reporter));
        Self { reporter }
    }
}

impl workspace::HookInitReporter for HookInitReporter {
    fn on_clone_start(&self, repo: &str) -> usize {
        self.reporter.set_root_prefix("Cloning repos...");

        self.reporter
            .on_start(format!("{} {}", "Cloning".bold().cyan(), repo.dimmed()))
    }

    fn on_clone_complete(&self, id: usize) {
        self.reporter.on_progress(id);
    }

    fn on_complete(&self) {
        self.reporter.on_complete();
    }
}

pub(crate) struct HookInstallReporter {
    reporter: Arc<ProgressReporter>,
}

impl HookInstallReporter {
    pub(crate) fn new(printer: Printer) -> Self {
        let reporter = Arc::new(ProgressReporter::from(printer));
        set_current_reporter(Some(&reporter));
        Self { reporter }
    }

    pub fn on_install_start(&self, hook: &Hook) -> usize {
        self.reporter.set_root_prefix("Installing hooks...");

        self.reporter.on_start(format!(
            "{} {}",
            "Installing".bold().cyan(),
            hook.id.dimmed(),
        ))
    }

    pub fn on_install_complete(&self, id: usize) {
        self.reporter.on_progress(id);
    }

    pub fn on_complete(&self) {
        self.reporter.on_complete();
    }
}

pub(crate) struct HookRunReporter {
    reporter: Arc<ProgressReporter>,
    dots: usize,
    show_project_headers: bool,
    running: Mutex<FxHashMap<usize, HookBar>>,
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
        let id = self.reporter.state.lock().unwrap().id();

        let progress_len = if len == 0 { 1 } else { len as u64 };
        let mut running = self.running.lock().unwrap();

        let mut groups = self.groups.lock().unwrap();
        let project_idx = hook.project().idx();
        let order = groups.len();
        if let Entry::Vacant(entry) = groups.entry(project_idx) {
            let header = if self.show_project_headers {
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
            } else {
                None
            };
            entry.insert(HookGroup::new(order, header));
        }
        for completed in
            self.collapse_to_fit_new_progress(&mut groups, Self::running_lines(&running), 1)
        {
            self.remove_hook_bar(completed);
        }
        let group = groups.get_mut(&project_idx).unwrap();
        let progress = if let Some(last_line) = &group.last_line {
            self.reporter.children.insert_after(
                last_line,
                ProgressBar::with_draw_target(Some(progress_len), self.reporter.printer.target()),
            )
        } else {
            self.reporter.children.insert_before(
                &self.reporter.root,
                ProgressBar::with_draw_target(Some(progress_len), self.reporter.printer.target()),
            )
        };
        group.last_line = Some(progress.clone());
        group.active_tail = Some(id);
        let label = if self.show_project_headers {
            format!("  {}", hook.name)
        } else {
            hook.name.clone()
        };
        let dots = self.dots.saturating_sub(label.width());
        progress.enable_steady_tick(Duration::from_millis(200));
        progress.set_style(
            ProgressStyle::with_template(&format!("{{msg}}{{bar:{dots}.green/dim}}"))
                .unwrap()
                .progress_chars(".."),
        );
        progress.set_message(label);
        running.insert(
            id,
            HookBar {
                hook_key: HookKey::from_hook(hook),
                progress,
                output_bars: Vec::new(),
                output_preview: OutputPreview::default(),
                passed: None,
            },
        );
        id
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
        let (project_idx, tail, running_lines, added_lines) = {
            let mut running = self.running.lock().unwrap();
            let (project_idx, tail, added_lines) = {
                let Some(run_bar) = running.get_mut(&id) else {
                    return;
                };

                let old_line_count = run_bar.output_bars.len();
                run_bar.output_preview.push_chunk(chunk);
                let line_count = run_bar.output_preview.visible_len();

                // Create preview rows only as output needs them, so short output does not
                // reserve the full rolling window height.
                while run_bar.output_bars.len() < line_count {
                    let tail = run_bar
                        .output_bars
                        .last()
                        .unwrap_or(&run_bar.progress)
                        .clone();
                    let output = self.reporter.children.insert_after(
                        &tail,
                        ProgressBar::with_draw_target(None, self.reporter.printer.target()),
                    );
                    output.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
                    output.set_message(HOOK_OUTPUT_PREVIEW_PREFIX.dimmed().to_string());
                    run_bar.output_bars.push(output);
                }

                // Keep the newest output visually anchored to the bottom of the preview window.
                let first_line = run_bar.output_bars.len().saturating_sub(line_count);
                let mut lines = run_bar.output_preview.visible_lines();
                for (idx, output_bar) in run_bar.output_bars.iter().enumerate() {
                    let line = if idx < first_line {
                        ""
                    } else {
                        lines.next().unwrap_or_default().trim_end()
                    };
                    let message = if width == 0 {
                        String::new()
                    } else {
                        format!(
                            "{HOOK_OUTPUT_PREVIEW_PREFIX}{}",
                            truncate_to_width(line, width)
                        )
                        .dimmed()
                        .to_string()
                    };
                    output_bar.set_message(message);
                }

                (
                    run_bar.hook_key.project_idx,
                    run_bar.output_bars.last().cloned(),
                    run_bar.output_bars.len() - old_line_count,
                )
            };

            // `run_bar`'s mutable borrow must end before counting all running lines.
            (
                project_idx,
                tail,
                Self::running_lines(&running),
                added_lines,
            )
        };

        if let Some(tail) = tail {
            let mut groups = self.groups.lock().unwrap();
            if let Some(group) = groups.get_mut(&project_idx)
                && group.active_tail == Some(id)
            {
                // New hooks in this project should be inserted after the active hook's preview.
                group.last_line = Some(tail);
            }
            if added_lines > 0 {
                // Growing preview rows may exceed the terminal height; hide old completed rows.
                let removed = self.collapse_to_fit_new_progress(&mut groups, running_lines, 0);
                drop(groups);
                for completed in removed {
                    self.remove_hook_bar(completed);
                }
            }
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
        if group.completed.visible_len() == 0 {
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

#[derive(Clone)]
pub(crate) struct AutoUpdateReporter {
    reporter: Arc<ProgressReporter>,
}

impl AutoUpdateReporter {
    pub(crate) fn new(printer: Printer) -> Self {
        let reporter = Arc::new(ProgressReporter::from(printer));
        set_current_reporter(Some(&reporter));
        Self { reporter }
    }
}

impl AutoUpdateReporter {
    pub fn on_update_start(&self, repo: &str) -> usize {
        self.reporter.set_root_prefix("Updating repos...");

        self.reporter
            .on_start(format!("{} {}", "Updating".bold().cyan(), repo.dimmed()))
    }

    pub fn on_update_complete(&self, id: usize) {
        self.reporter.on_progress(id);
    }

    pub fn on_complete(&self) {
        self.reporter.on_complete();
    }
}

#[derive(Debug)]
pub(crate) struct CleaningReporter {
    bar: ProgressBar,
}

impl CleaningReporter {
    pub(crate) fn new(printer: Printer, max: usize) -> Self {
        let bar = ProgressBar::with_draw_target(Some(max as u64), printer.target());
        bar.set_style(
            ProgressStyle::with_template("{prefix} [{bar:20}] {percent}%")
                .unwrap()
                .progress_chars("=> "),
        );
        bar.set_prefix(format!("{}", "Cleaning".bold().cyan()));
        Self { bar }
    }
}

impl CleaningReporter {
    pub(crate) fn on_clean(&self) {
        self.bar.inc(1);
    }

    pub(crate) fn on_complete(&self) {
        self.bar.finish_and_clear();
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

    fn preview_lines(preview: &OutputPreview) -> Vec<&str> {
        preview.visible_lines().collect()
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

        assert_eq!(preview_lines(&preview), vec!["processing file"]);
    }

    #[test]
    fn output_preview_handles_split_crlf() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"processing file\r");
        preview.push_chunk(b"\n");

        assert_eq!(preview_lines(&preview), vec!["processing file"]);
    }

    #[test]
    fn output_preview_replaces_carriage_return_line() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"first\rsecond");

        assert_eq!(preview_lines(&preview), vec!["second"]);
    }

    #[test]
    fn output_preview_strips_ansi_codes() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"\x1b[31mred\x1b[0m\n");

        assert_eq!(preview_lines(&preview), vec!["red"]);
    }

    #[test]
    fn output_preview_keeps_last_preview_window() {
        let mut preview = OutputPreview::default();

        preview.push_chunk(b"one\ntwo\nthree\nfour\n");

        assert_eq!(preview_lines(&preview), vec!["two", "three", "four"]);
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
        assert_eq!(completed.visible_len(), 1);
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 2 hooks hidden: 1 passed, 1 failed")
        );

        let removed = completed.hide_one_line();
        assert_eq!(removed.len(), 1);
        assert_eq!(completed.visible_len(), 0);
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
