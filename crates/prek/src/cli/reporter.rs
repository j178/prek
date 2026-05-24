use std::borrow::Cow;
use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use console::Term;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use rustc_hash::FxHashMap;
use unicode_width::UnicodeWidthStr;

use crate::hook::Hook;
use crate::printer::Printer;
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
    passed: Option<bool>,
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

    fn hide_visible(&mut self) -> VecDeque<HookBar> {
        let mut removed = VecDeque::new();
        let mut visible = VecDeque::new();
        while let Some(completed) = self.visible.pop_front() {
            if completed.passed.is_some() {
                self.hide_completed(&completed);
                removed.push_back(completed);
            } else {
                visible.push_back(completed);
            }
        }
        self.visible = visible;
        removed
    }

    fn hide_completed(&mut self, completed: &HookBar) {
        match completed.passed {
            Some(true) => self.hidden_passed += 1,
            Some(false) => self.hidden_failed += 1,
            None => {}
        }
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

    fn has_hideable_visible(&self) -> bool {
        self.visible
            .iter()
            .any(|completed| completed.passed.is_some())
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

        if self.hidden_passed == hidden {
            return Some(format!("⋮ {hidden} hooks hidden: all passed"));
        }
        if self.hidden_failed == hidden {
            return Some(format!("⋮ {hidden} hooks hidden: all failed"));
        }

        let mut statuses = Vec::new();
        if self.hidden_passed > 0 {
            statuses.push(format!("{} passed", self.hidden_passed));
        }
        if self.hidden_failed > 0 {
            statuses.push(format!("{} failed", self.hidden_failed));
        }
        Some(format!("⋮ {hidden} hooks hidden: {}", statuses.join(", ")))
    }

    fn clear(&mut self) -> VecDeque<HookBar> {
        self.hidden_passed = 0;
        self.hidden_failed = 0;
        std::mem::take(&mut self.visible)
    }
}

#[derive(Debug)]
struct ProjectBar {
    order: usize,
    finished: bool,
    header: ProgressBar,
    last_line: ProgressBar,
    hidden_summary: Option<ProgressBar>,
    completed: CompletedBars,
}

pub(crate) fn project_status_marker(failed: bool) -> String {
    if failed {
        "×".red().to_string()
    } else {
        "✓".green().to_string()
    }
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
    completed: Mutex<CompletedBars>,
    projects: Mutex<FxHashMap<usize, ProjectBar>>,
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
            completed: Mutex::default(),
            projects: Mutex::default(),
        }
    }

    fn update_project_summary(&self, project: &mut ProjectBar) {
        let Some(message) = project.completed.hidden_summary() else {
            return;
        };

        let summary = if let Some(summary) = &project.hidden_summary {
            summary.clone()
        } else {
            let summary = self.reporter.children.insert_after(
                &project.header,
                ProgressBar::with_draw_target(None, self.reporter.printer.target()),
            );
            summary.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
            project.hidden_summary = Some(summary.clone());
            summary
        };
        if project.completed.visible_len() == 0 {
            project.last_line = summary.clone();
        }
        summary.set_message(format!("  {}", message.dimmed()));
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

    fn active_project_lines(projects: &FxHashMap<usize, ProjectBar>, running: usize) -> usize {
        1 + running
            + projects
                .values()
                .map(|project| {
                    1 + project.completed.visible_len()
                        + usize::from(project.completed.is_collapsed())
                })
                .sum::<usize>()
    }

    fn collapse_completed_projects(
        &self,
        projects: &mut FxHashMap<usize, ProjectBar>,
        running: usize,
    ) -> Vec<HookBar> {
        let Some(limit) = self.progress_line_limit() else {
            return Vec::new();
        };

        let mut removed = Vec::new();
        while Self::active_project_lines(projects, running) > limit {
            let Some(project_idx) = projects
                .iter()
                .filter(|(_, project)| project.finished && project.completed.has_hideable_visible())
                .min_by_key(|(_, project)| project.order)
                .map(|(project_idx, _)| *project_idx)
            else {
                break;
            };

            let Some(project) = projects.get_mut(&project_idx) else {
                break;
            };
            removed.extend(project.completed.hide_visible());
            self.update_project_summary(project);
        }

        removed
    }

    fn remember_completed(&self, completed: HookBar, running: usize) {
        let trimmed = if self.show_project_headers {
            let mut projects = self.projects.lock().unwrap();
            if let Some(project) = projects.get_mut(&completed.hook_key.project_idx) {
                project.completed.push(completed);
                self.update_project_summary(project);
                self.collapse_completed_projects(&mut projects, running)
            } else {
                vec![completed]
            }
        } else {
            self.completed.lock().unwrap().push(completed);
            Vec::new()
        };

        for completed in trimmed {
            self.reporter.children.remove(&completed.progress);
        }
    }

    pub fn on_run_result(&self, hook: &Hook, passed: bool) {
        let hook_key = HookKey::from_hook(hook);
        let progress = if self.show_project_headers {
            let running = self.running.lock().unwrap().len();
            let mut projects = self.projects.lock().unwrap();
            let Some(project) = projects.get_mut(&hook_key.project_idx) else {
                return;
            };
            let progress = project.completed.record_result(hook_key, passed);
            self.update_project_summary(project);
            let trimmed = self.collapse_completed_projects(&mut projects, running);
            drop(projects);
            for completed in trimmed {
                self.reporter.children.remove(&completed.progress);
            }
            progress
        } else {
            let mut completed = self.completed.lock().unwrap();
            completed.record_result(hook_key, passed)
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
        let running = self.running.lock().unwrap().len();
        let mut projects = self.projects.lock().unwrap();
        let Some(project_bar) = projects.get_mut(&project.idx()) else {
            return;
        };
        project_bar.finished = true;
        let header = &project_bar.header;
        header.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
        header.set_message(format!(
            "{} {}",
            project_status_marker(failed),
            project.display_name().cyan().bold()
        ));

        header.finish();

        let trimmed = self.collapse_completed_projects(&mut projects, running);
        drop(projects);
        for completed in trimmed {
            self.reporter.children.remove(&completed.progress);
        }
    }

    pub fn on_run_start(&self, hook: &Hook, len: usize) -> usize {
        let id = self.reporter.state.lock().unwrap().id();

        let progress_len = if len == 0 { 1 } else { len as u64 };

        let (progress, label) = if self.show_project_headers {
            let mut projects = self.projects.lock().unwrap();
            let order = projects.len();
            let project_bar = projects.entry(hook.project().idx()).or_insert_with(|| {
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
                ProjectBar {
                    order,
                    finished: false,
                    last_line: header.clone(),
                    header,
                    hidden_summary: None,
                    completed: CompletedBars::default(),
                }
            });
            let progress = self.reporter.children.insert_after(
                &project_bar.last_line,
                ProgressBar::with_draw_target(Some(progress_len), self.reporter.printer.target()),
            );
            project_bar.last_line = progress.clone();
            (progress, format!("  {}", hook.name))
        } else {
            let progress = self.reporter.children.insert_before(
                &self.reporter.root,
                ProgressBar::with_draw_target(Some(progress_len), self.reporter.printer.target()),
            );
            (progress, hook.name.clone())
        };

        let dots = self.dots.saturating_sub(label.width());
        progress.enable_steady_tick(Duration::from_millis(200));
        progress.set_style(
            ProgressStyle::with_template(&format!("{{msg}}{{bar:{dots}.green/dim}}"))
                .unwrap()
                .progress_chars(".."),
        );
        progress.set_message(label);
        self.running.lock().unwrap().insert(
            id,
            HookBar {
                hook_key: HookKey::from_hook(hook),
                progress,
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

    pub fn on_run_complete(&self, id: usize) {
        let (running, running_count) = {
            let mut running = self.running.lock().unwrap();
            let completed = running.remove(&id).unwrap();
            let running_count = running.len();
            (completed, running_count)
        };
        self.reporter.root.inc(1);

        // Keep the completed line visible until the group result is rendered.
        let progress = &running.progress;
        progress.set_position(progress.length().unwrap_or(1));
        progress.finish();
        self.remember_completed(running, running_count);
    }

    pub fn clear_completed(&self) {
        let standalone_completed = {
            let mut completed = self.completed.lock().unwrap();
            completed.clear()
        };
        let projects = {
            let mut projects = self.projects.lock().unwrap();
            projects
                .drain()
                .map(|(_, project)| project)
                .collect::<Vec<_>>()
        };

        for completed in standalone_completed {
            self.reporter.children.remove(&completed.progress);
        }

        for mut project in projects {
            self.reporter.children.remove(&project.header);
            if let Some(summary) = project.hidden_summary {
                self.reporter.children.remove(&summary);
            }
            for completed in project.completed.clear() {
                self.reporter.children.remove(&completed.progress);
            }
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

    #[test]
    fn hidden_summary_shows_total_and_result_breakdown() {
        let completed = CompletedBars {
            hidden_passed: 8,
            ..CompletedBars::default()
        };
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 8 hooks hidden: all passed")
        );

        let completed = CompletedBars {
            hidden_failed: 8,
            ..CompletedBars::default()
        };
        assert_eq!(
            completed.hidden_summary().as_deref(),
            Some("⋮ 8 hooks hidden: all failed")
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
}
