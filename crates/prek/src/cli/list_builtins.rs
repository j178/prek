use std::fmt::Write;

use owo_colors::OwoColorize;
use serde::Serialize;
use strum::IntoEnumIterator;

use crate::cli::{ExitStatus, ListOutputFormat};
use crate::config::BuiltinHook;
use crate::hooks::BuiltinHooks;
use crate::printer::Printer;

#[derive(Serialize)]
struct SerializableBuiltinHook {
    id: String,
    name: String,
    description: Option<String>,
}

pub(crate) fn list_builtins(
    output_format: ListOutputFormat,
    verbose: bool,
    printer: Printer,
) -> anyhow::Result<ExitStatus> {
    let hooks: Vec<_> = BuiltinHooks::iter()
        .filter_map(|variant| {
            let id = variant.as_ref();
            BuiltinHook::from_id(id).ok()
        })
        .collect();

    match output_format {
        ListOutputFormat::Text => {
            if verbose {
                for hook in &hooks {
                    writeln!(printer.stdout(), "{}", hook.id.bold())?;
                    if let Some(description) = &hook.options.description {
                        writeln!(printer.stdout(), "  {description}")?;
                    }
                    writeln!(printer.stdout())?;
                }
            } else {
                for hook in &hooks {
                    writeln!(printer.stdout(), "{}", hook.id)?;
                }
            }
        }
        ListOutputFormat::Json => {
            let serializable: Vec<_> = hooks
                .into_iter()
                .map(|h| SerializableBuiltinHook {
                    id: h.id,
                    name: h.name,
                    description: h.options.description,
                })
                .collect();
            let json_output = serde_json::to_string_pretty(&serializable)?;
            writeln!(printer.stdout(), "{json_output}")?;
        }
    }

    Ok(ExitStatus::Success)
}
