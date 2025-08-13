use owo_colors::OwoColorize;
use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::PathBuf;

use crate::cli::ExitStatus;
use crate::cli::reporter::HookInitReporter;
use crate::config::{Language, Stage};
use crate::printer::Printer;
use crate::store::STORE;
use crate::workspace::Project;

pub(crate) async fn list(
    config: Option<PathBuf>,
    verbose: bool,
    hook_ids: Vec<String>,
    hook_stage: Option<Stage>,
    language: Option<Language>,
    printer: Printer,
) -> anyhow::Result<ExitStatus> {
    let mut project = Project::from_config_file(config)?;
    let store = STORE.as_ref()?;

    let reporter = HookInitReporter::from(printer);

    let lock = store.lock_async().await?;
    let hooks = project.init_hooks(store, Some(&reporter)).await?;
    drop(lock);

    let hook_ids = hook_ids.into_iter().collect::<BTreeSet<_>>();
    let hooks: Vec<_> = hooks
        .into_iter()
        .filter(|h| hook_ids.is_empty() || hook_ids.contains(&h.id) || hook_ids.contains(&h.alias))
        .filter(|h| hook_stage.is_none_or(|hook_stage| h.stages.contains(hook_stage)))
        .filter(|h| language.is_none_or(|lang| h.language == lang))
        .collect();

    if verbose {
        // TODO: show repo path and environment path (if installed)
        for hook in &hooks {
            writeln!(printer.stdout(), "{}", hook.id.bold())?;

            if !hook.alias.is_empty() && hook.alias != hook.id {
                writeln!(
                    printer.stdout(),
                    "  {} {}",
                    "Alias:".bold().cyan(),
                    hook.alias
                )?;
            }
            writeln!(
                printer.stdout(),
                "  {} {}",
                "Name:".bold().cyan(),
                hook.name
            )?;
            if let Some(description) = &hook.description {
                writeln!(
                    printer.stdout(),
                    "  {} {}",
                    "Description:".bold().cyan(),
                    description
                )?;
            }
            writeln!(
                printer.stdout(),
                "  {} {}",
                "Language:".bold().cyan(),
                hook.language.as_str()
            )?;
            writeln!(
                printer.stdout(),
                "  {} {}",
                "Stages:".bold().cyan(),
                hook.stages
            )?;

            writeln!(printer.stdout())?;
        }
    } else {
        for hook in &hooks {
            writeln!(printer.stdout(), "{}", hook.id)?;
        }
    }

    Ok(ExitStatus::Success)
}
