use clap::ValueEnum;
use owo_colors::OwoColorize;
use serde::Serialize;
use std::collections::BTreeSet;
use std::fmt::Write;
use std::path::PathBuf;

use crate::cli::reporter::HookInitReporter;
use crate::cli::{ExitStatus, ListOutputFormat};
use crate::config::{Language, Stage};
use crate::fs::CWD;
use crate::hook;
use crate::printer::Printer;
use crate::store::STORE;
use crate::workspace::Project;

#[derive(Serialize)]
struct SerializableHook {
    id: String,
    name: String,
    alias: String,
    language: Language,
    description: Option<String>,
    stages: Vec<Stage>,
}

pub(crate) async fn list(
    config: Option<PathBuf>,
    verbose: bool,
    hook_ids: Vec<String>,
    hook_stage: Option<Stage>,
    language: Option<Language>,
    output_format: ListOutputFormat,
    printer: Printer,
) -> anyhow::Result<ExitStatus> {
    let mut project = Project::from_config_file_or_directory(config, &CWD)?;
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

    match output_format {
        ListOutputFormat::Text => {
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
        }
        ListOutputFormat::Json => {
            let serializable_hooks: Vec<_> = hooks
                .into_iter()
                .map(|h| {
                    let stages = match h.stages {
                        hook::Stages::All => Stage::value_variants().to_vec(),
                        hook::Stages::Some(s) => s.into_iter().collect(),
                    };
                    SerializableHook {
                        id: h.id,
                        name: h.name,
                        alias: h.alias,
                        language: h.language,
                        description: h.description,
                        stages,
                    }
                })
                .collect();

            let json_output = serde_json::to_string_pretty(&serializable_hooks)?;
            writeln!(printer.stdout(), "{json_output}")?;
        }
    }

    Ok(ExitStatus::Success)
}
