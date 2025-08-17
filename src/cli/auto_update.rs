use std::path::PathBuf;

use anyhow::Result;

use crate::cli::ExitStatus;
use crate::printer::Printer;

pub(crate) async fn auto_update(
    config: Option<PathBuf>,
    repos: Vec<String>,
    bleeding_edge: bool,
    freeze: bool,
    jobs: usize,
    printer: Printer,
) -> Result<ExitStatus> {
    // TODO: update whole workspace
    let mut project = Project::from_config_file_or_directory(config, &CWD)?;
    let store = STORE.as_ref()?;
    let _lock = store.lock_async().await?;

    let reporter = AutoUpdateReporter::from(printer);
    project.auto_update(store, Some(&reporter)).await?;

    Ok(ExitStatus::Success)
}
