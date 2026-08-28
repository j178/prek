use owo_colors::{Style, Styled};

pub(crate) use filter::{
    CollectOptions, FileSelection, FileTagCache, FileTagFilter, HookFileFilter, ProjectFiles,
    RunFileIndex, RunInput, collect_run_input,
};
pub(crate) use install::{InstallCache, install_hooks};
pub(crate) use reporter::{HookRunReporter, project_status_marker};
pub(crate) use run::{HideStatus, run};
pub(crate) use selector::{ConfiguredHook, GroupFilters, SelectorSource, Selectors};

mod diff;
mod filter;
mod install;
mod keeper;
mod reporter;
#[allow(clippy::module_inception)]
mod run;
mod selector;

const PASSED: Styled<&str> = Style::new().green().reversed().style("Passed");
const FAILED: Styled<&str> = Style::new().red().reversed().style("Failed");
const SKIPPED: Styled<&str> = Style::new().cyan().reversed().style("Skipped");
const DRY_RUN: Styled<&str> = Style::new().yellow().reversed().style("Dry Run");
