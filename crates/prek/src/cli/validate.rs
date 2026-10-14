use std::error::Error;
use std::fmt::Write;
use std::iter;
use std::path::PathBuf;

use anyhow::Result;
use itertools::Itertools;
use owo_colors::OwoColorize;

use crate::cli::ExitStatus;
use crate::config::{read_config, read_manifest};
use crate::printer::Printer;
use crate::warn_user;

pub(crate) fn validate_configs(configs: Vec<PathBuf>, printer: Printer) -> Result<ExitStatus> {
    let mut status = ExitStatus::Success;

    if configs.is_empty() {
        warn_user!("No configs to check");
        return Ok(ExitStatus::Success);
    }

    for config in configs {
        if let Err(err) = read_config(&config) {
            writeln!(printer.stderr(), "{}: {}", "error".red().bold(), err)?;
            // YAML diagnostics already include their sources.
            // TODO: Remove after https://github.com/bourumir-wyngs/serde-saphyr/issues/227.
            for source in
                iter::successors(err.source(), |&err| err.source()).take_while_inclusive(|err| {
                    !err.is::<serde_saphyr::Error>() && !err.is::<Box<serde_saphyr::Error>>()
                })
            {
                writeln!(
                    printer.stderr(),
                    "  {}: {}",
                    "caused by".red().bold(),
                    source
                )?;
            }
            status = ExitStatus::Failure;
        }
    }

    if status == ExitStatus::Success {
        writeln!(
            printer.stderr(),
            "{}: All configs are valid",
            "success".green().bold()
        )?;
    }

    Ok(status)
}

pub(crate) fn validate_manifest(manifests: Vec<PathBuf>, printer: Printer) -> Result<ExitStatus> {
    let mut status = ExitStatus::Success;

    if manifests.is_empty() {
        warn_user!("No manifests to check");
        return Ok(ExitStatus::Success);
    }

    for manifest in manifests {
        if let Err(err) = read_manifest(&manifest) {
            writeln!(printer.stderr(), "{}: {}", "error".red().bold(), err)?;
            // YAML diagnostics already include their sources.
            // TODO: Remove after https://github.com/bourumir-wyngs/serde-saphyr/issues/227.
            for source in
                iter::successors(err.source(), |&err| err.source()).take_while_inclusive(|err| {
                    !err.is::<serde_saphyr::Error>() && !err.is::<Box<serde_saphyr::Error>>()
                })
            {
                writeln!(
                    printer.stderr(),
                    "  {}: {}",
                    "caused by".red().bold(),
                    source
                )?;
            }
            status = ExitStatus::Failure;
        }
    }

    if status == ExitStatus::Success {
        writeln!(
            printer.stderr(),
            "{}: All manifests are valid",
            "success".green().bold()
        )?;
    }

    Ok(status)
}
