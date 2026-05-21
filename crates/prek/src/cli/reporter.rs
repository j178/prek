use std::borrow::Cow;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

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
    /// Completed run bars that should stay visible until the current group is rendered.
    completed: Vec<ProgressBar>,
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

#[derive(Debug, Clone)]
struct ProjectBar {
    header: ProgressBar,
    tail: ProgressBar,
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

    fn on_complete(&self) {
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
            ProgressStyle::with_template("{spinner:.white} {msg:.dim}")
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
        self.reporter
            .root
            .set_message(format!("{}", "Cloning repos...".bold().cyan()));

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
        self.reporter
            .root
            .set_message(format!("{}", "Installing hooks...".bold().cyan()));

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
    projects: Mutex<FxHashMap<usize, ProjectBar>>,
}

impl HookRunReporter {
    pub fn new(printer: Printer, dots: usize, show_project_headers: bool) -> Self {
        let reporter = Arc::new(ProgressReporter::from(printer));
        set_current_reporter(Some(&reporter));

        Self {
            reporter,
            dots,
            show_project_headers,
            projects: Mutex::default(),
        }
    }

    pub fn on_project_complete(&self, project: &workspace::Project) {
        let Some(project_bar) = self.projects.lock().unwrap().remove(&project.idx()) else {
            return;
        };
        let header = project_bar.header;
        header.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
        header.set_message(format!(
            "{} {}",
            "✓".green(),
            project.display_name().cyan().bold()
        ));

        self.reporter
            .state
            .lock()
            .unwrap()
            .completed
            .push(header.clone());
        header.finish();
    }

    pub fn on_run_start(&self, hook: &Hook, len: usize) -> usize {
        self.reporter
            .root
            .set_message(format!("{}", "Running hooks...".bold().cyan()));

        let id = self.reporter.state.lock().unwrap().id();

        let progress_len = if len == 0 { 1 } else { len as u64 };

        let (progress, label) = if self.show_project_headers {
            let mut projects = self.projects.lock().unwrap();
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
                    header: header.clone(),
                    tail: header,
                }
            });
            let progress = self.reporter.children.insert_after(
                &project_bar.tail,
                ProgressBar::with_draw_target(Some(progress_len), self.reporter.printer.target()),
            );
            project_bar.tail = progress.clone();
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
        self.reporter
            .state
            .lock()
            .unwrap()
            .bars
            .insert(id, progress);
        id
    }

    pub fn on_run_progress(&self, id: usize, completed: u64) {
        let state = self.reporter.state.lock().unwrap();
        let progress = &state.bars[&id];
        progress.inc(completed);
    }

    pub fn on_run_complete(&self, id: usize, passed: bool) {
        let progress = {
            let mut state = self.reporter.state.lock().unwrap();
            let progress = state.bars.remove(&id).unwrap();
            state.completed.push(progress.clone());
            progress
        };
        let label = progress.message();
        self.reporter.root.inc(1);

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

        // Keep the finished line visible until the group result is rendered.
        progress.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
        progress.set_message(format!("{label}{dots}{status}"));
        progress.finish();
    }

    pub fn clear_completed(&self) {
        let completed = {
            let mut state = self.reporter.state.lock().unwrap();
            std::mem::take(&mut state.completed)
        };

        for progress in completed {
            self.reporter.children.remove(&progress);
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
        let project_bars = {
            let mut projects = self.projects.lock().unwrap();
            std::mem::take(&mut *projects)
        };
        for project_bar in project_bars.into_values() {
            self.reporter.children.remove(&project_bar.header);
        }
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
        self.reporter
            .root
            .set_message(format!("{}", "Updating repos...".bold().cyan()));

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
