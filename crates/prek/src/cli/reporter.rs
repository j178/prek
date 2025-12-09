use std::borrow::Cow;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;
use rustc_hash::FxHashMap;

use crate::hook::Hook;
use crate::printer::Printer;
use crate::workspace;

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

pub(crate) struct HookInitReporter {
    reporter: ProgressReporter,
}

impl From<Printer> for HookInitReporter {
    fn from(printer: Printer) -> Self {
        let multi = MultiProgress::with_draw_target(printer.target());
        let root = multi.add(ProgressBar::with_draw_target(None, printer.target()));
        root.enable_steady_tick(Duration::from_millis(200));
        root.set_style(
            ProgressStyle::with_template("{spinner:.white} {msg:.dim}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );

        let reporter = ProgressReporter::new(root, multi, printer);
        Self { reporter }
    }
}

impl workspace::HookInitReporter for HookInitReporter {
    fn on_clone_start(&self, repo: &str) -> usize {
        self.reporter
            .root
            .set_message(format!("{}", "Initializing hooks...".bold().cyan()));

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
    reporter: ProgressReporter,
}

impl From<Printer> for HookInstallReporter {
    fn from(printer: Printer) -> Self {
        let multi = MultiProgress::with_draw_target(printer.target());
        let root = multi.add(ProgressBar::with_draw_target(None, printer.target()));
        root.enable_steady_tick(Duration::from_millis(200));
        root.set_style(
            ProgressStyle::with_template("{spinner:.white} {msg:.dim}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );

        let reporter = ProgressReporter::new(root, multi, printer);
        Self { reporter }
    }
}

impl HookInstallReporter {
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

pub(crate) struct AutoUpdateReporter {
    reporter: ProgressReporter,
}

impl From<Printer> for AutoUpdateReporter {
    fn from(printer: Printer) -> Self {
        let multi = MultiProgress::with_draw_target(printer.target());
        let root = multi.add(ProgressBar::with_draw_target(None, printer.target()));
        root.enable_steady_tick(Duration::from_millis(200));
        root.set_style(
            ProgressStyle::with_template("{spinner:.white} {msg:.dim}")
                .unwrap()
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );

        let reporter = ProgressReporter::new(root, multi, printer);
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
