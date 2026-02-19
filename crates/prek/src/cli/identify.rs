use std::fmt::Write;
use std::path::PathBuf;

use itertools::Itertools;
use owo_colors::OwoColorize;
use prek_identify::tags_from_path;
use serde::Serialize;

use crate::cli::{ExitStatus, IdentifyOutputFormat};
use crate::printer::Printer;

#[derive(Serialize)]
struct IdentifyEntry {
    path: String,
    tags: Vec<String>,
}

pub(crate) fn identify(
    paths: &[PathBuf],
    output_format: IdentifyOutputFormat,
    printer: Printer,
) -> anyhow::Result<ExitStatus> {
    let mut status = ExitStatus::Success;
    let mut outputs = Vec::new();

    for path in paths {
        match tags_from_path(path) {
            Ok(tags) => match output_format {
                IdentifyOutputFormat::Text => {
                    writeln!(
                        printer.stdout_important(),
                        "{}: {}",
                        path.display().bold(),
                        tags.iter().join(", ")
                    )?;
                }
                IdentifyOutputFormat::Json => {
                    outputs.push(IdentifyEntry {
                        path: path.display().to_string(),
                        tags: tags.iter().map(ToString::to_string).collect(),
                    });
                }
            },
            Err(err) => {
                status = ExitStatus::Failure;
                writeln!(
                    printer.stderr(),
                    "{}: {}: {}",
                    "error".red().bold(),
                    path.display(),
                    err
                )?;
            }
        }
    }

    if matches!(output_format, IdentifyOutputFormat::Json) {
        let json_output = serde_json::to_string_pretty(&outputs)?;
        writeln!(printer.stdout_important(), "{json_output}")?;
    }

    Ok(status)
}
