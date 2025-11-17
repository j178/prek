use std::fmt::Write;
use std::path::PathBuf;

use anyhow::Context;
use clap::ValueEnum;
use owo_colors::OwoColorize;
use serde::Serialize;

use crate::cli::reporter::HookInitReporter;
use crate::cli::run::Selectors;
use crate::cli::{ExitStatus, ListOutputFormat};
use crate::config::{Language, Stage};
use crate::fs::CWD;
use crate::hook;
use crate::printer::Printer;
use crate::store::Store;
use crate::workspace::Workspace;

#[derive(Serialize)]
struct SerializableHook {
    id: String,
    full_id: String,
    name: String,
    alias: String,
    language: Language,
    description: Option<String>,
    stages: Vec<Stage>,
}

pub(crate) async fn list(
    store: &Store,
    config: Option<PathBuf>,
    includes: Vec<String>,
    skips: Vec<String>,
    hook_stage: Option<Stage>,
    language: Option<Language>,
    output_format: ListOutputFormat,
    refresh: bool,
    verbose: bool,
    printer: Printer,
) -> anyhow::Result<ExitStatus> {
    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    let selectors = Selectors::load(&includes, &skips, &workspace_root)?;
    let mut workspace =
        Workspace::discover(store, workspace_root, config, Some(&selectors), refresh)?;

    let reporter = HookInitReporter::from(printer);
    let lock = store.lock_async().await?;
    let hooks = workspace
        .init_hooks(store, Some(&reporter))
        .await
        .context("Failed to init hooks")?;

    drop(lock);

    let filtered_hooks: Vec<_> = hooks
        .into_iter()
        .filter(|h| selectors.matches_hook(h))
        .filter(|h| hook_stage.is_none_or(|hook_stage| h.stages.contains(hook_stage)))
        .filter(|h| language.is_none_or(|lang| h.language == lang))
        .collect();

    selectors.report_unused();

    match output_format {
        ListOutputFormat::Text => {
            if verbose {
                // TODO: show repo path and environment path (if installed)
                for hook in &filtered_hooks {
                    writeln!(printer.stdout(), "{}", hook.full_id().bold())?;

                    writeln!(printer.stdout(), "  {} {}", "ID:".bold().cyan(), hook.id)?;
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
                // TODO: add project prefix to hook id
                for hook in &filtered_hooks {
                    writeln!(printer.stdout(), "{}", hook.full_id())?;
                }
            }
        }
        ListOutputFormat::Json => {
            let serializable_hooks: Vec<_> = filtered_hooks
                .into_iter()
                .map(|h| {
                    let id = h.id.clone();
                    let full_id = h.full_id();
                    let stages = match h.stages {
                        hook::Stages::All => Stage::value_variants().to_vec(),
                        hook::Stages::Some(s) => s.into_iter().collect(),
                    };
                    SerializableHook {
                        id,
                        full_id,
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
