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

/// List all builtin hooks.
pub(crate) fn list_builtins(
    output_format: ListOutputFormat,
    printer: Printer,
) -> anyhow::Result<ExitStatus> {
    let hooks = BuiltinHooks::iter().map(|variant| {
        let id = variant.as_ref();
        let hook = BuiltinHook::from_id(id).expect("All BuiltinHooks variants should be valid");
        (variant, hook)
    });

    let mut stdout = printer.stdout_important();
    match output_format {
        ListOutputFormat::Text => {
            for (variant, hook) in hooks {
                writeln!(stdout, "{}", hook.id.bold())?;
                if let Some(description) = &hook.options.description {
                    writeln!(stdout, "  {description}")?;
                }
                if let Some(flags_help) = variant.flags_help() {
                    writeln!(stdout, "  flags:")?;
                    for line in flags_help.lines() {
                        writeln!(stdout, "  {line}")?;
                    }
                }
                writeln!(stdout)?;
            }
        }
        ListOutputFormat::Json => {
            let serializable: Vec<_> = hooks
                .map(|(_, h)| SerializableBuiltinHook {
                    id: h.id,
                    name: h.name,
                    description: h.options.description,
                })
                .collect();
            let json_output = serde_json::to_string_pretty(&serializable)?;
            writeln!(stdout, "{json_output}")?;
        }
    }

    Ok(ExitStatus::Success)
}
