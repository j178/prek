use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use indicatif::{MultiProgress, ProgressBar, ProgressStyle};
use owo_colors::OwoColorize;

use crate::hook;
use crate::hook::Hook;
use crate::printer::Printer;

#[derive(Default, Debug)]
struct BarState {
    /// A map of progress bars, by ID.
    bars: HashMap<usize, ProgressBar>,
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

struct SubProgress {
    state: Arc<Mutex<BarState>>,
    multi: MultiProgress,
}

pub(crate) struct HookInitReporter {
    printer: Printer,
    root: ProgressBar,
    sub: SubProgress,
}

impl HookInitReporter {
    pub(crate) fn new(root: ProgressBar, multi_progress: MultiProgress, printer: Printer) -> Self {
        Self {
            printer,
            root,
            sub: SubProgress {
                state: Arc::default(),
                multi: multi_progress,
            },
        }
    }
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
        root.set_message("Initializing hooks...");
        Self::new(root, multi, printer)
    }
}

impl hook::HookInitReporter for HookInitReporter {
    fn on_repo_clone_start(&self, repo: &str) -> usize {
        let mut state = self.sub.state.lock().unwrap();
        let id = state.id();

        let progress = self.sub.multi.insert_before(
            &self.root,
            ProgressBar::with_draw_target(None, self.printer.target()),
        );

        progress.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
        progress.set_message(format!("{} {}", "Cloning".bold().cyan(), repo.dimmed(),));

        state.bars.insert(id, progress);
        id
    }

    fn on_repo_clone_complete(&self, id: usize) {
        let progress = {
            let mut state = self.sub.state.lock().unwrap();
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

pub struct HookInstallReporter {
    printer: Printer,
    root: ProgressBar,
    sub: SubProgress,
}

impl HookInstallReporter {
    pub fn new(root: ProgressBar, multi_progress: MultiProgress, printer: Printer) -> Self {
        Self {
            printer,
            root,
            sub: SubProgress {
                state: Arc::default(),
                multi: multi_progress,
            },
        }
    }

    pub fn on_install_start(&self, hook: &Hook) -> usize {
        let mut state = self.sub.state.lock().unwrap();
        let id = state.id();

        let progress = self.sub.multi.insert_before(
            &self.root,
            ProgressBar::with_draw_target(None, self.printer.target()),
        );

        progress.set_style(ProgressStyle::with_template("{wide_msg}").unwrap());
        progress.set_message(format!(
            "{} {}",
            "Installing".bold().cyan(),
            hook.id.dimmed(),
        ));

        state.bars.insert(id, progress);
        id
    }

    pub fn on_install_complete(&self, id: usize) {
        let progress = {
            let mut state = self.sub.state.lock().unwrap();
            state.bars.remove(&id).unwrap()
        };

        self.root.inc(1);
        progress.finish_and_clear();
    }

    pub fn on_complete(&self) {
        self.root.set_message("");
        self.root.finish_and_clear();
    }
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
        root.set_message("Installing hooks...");
        Self::new(root, multi, printer)
    }
}
