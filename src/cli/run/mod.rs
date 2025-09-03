pub(crate) use filter::{CollectOptions, FileFilter, collect_files};
pub(crate) use run::{install_hooks, run};
pub(crate) use selection::{Selections, get_skips};

mod filter;
mod keeper;
#[allow(clippy::module_inception)]
mod run;
mod selection;
