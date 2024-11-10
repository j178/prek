use std::fmt::Write;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::ExitStatus;
use crate::fs::Simplified;
use crate::printer::Printer;
use crate::store::Store;

pub(crate) fn clean(printer: Printer) -> Result<ExitStatus> {
    let store = Store::from_settings()?;

    if !store.path().exists() {
        writeln!(printer.stdout(), "Nothing to clean")?;
        return Ok(ExitStatus::Success);
    }

    fs_err::remove_dir_all(store.path())?;
    writeln!(
        printer.stdout(),
        "Cleaned `{}`",
        store.path().user_display().cyan()
    )?;

    Ok(ExitStatus::Success)
}
