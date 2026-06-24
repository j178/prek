use std::error::Error;
use std::fmt::Write;
use std::iter;
use std::path::PathBuf;

use anyhow::Result;
use owo_colors::OwoColorize;

use crate::cli::ExitStatus;
use crate::config::{Config, read_config, read_manifest};
use crate::printer::Printer;
use crate::warn_user;

fn write_unfrozen_rev_error(
    config: &Config,
    config_path: &std::path::Path,
    printer: Printer,
) -> Result<bool> {
    let repos = config.repos_with_unfrozen_revs().collect::<Vec<_>>();
    if repos.is_empty() {
        return Ok(false);
    }

    writeln!(
        printer.stderr(),
        "{}: Config `{}` contains non-frozen remote hook revisions",
        "error".red().bold(),
        config_path.display()
    )?;
    for repo in repos {
        writeln!(
            printer.stderr(),
            "  - {} uses rev `{}`",
            repo.repo.cyan(),
            repo.rev.yellow()
        )?;
    }
    writeln!(
        printer.stderr(),
        "{}: run `{}` to replace tags with commit SHAs",
        "hint".yellow().bold(),
        "prek auto-update --freeze".cyan()
    )?;

    Ok(true)
}

pub(crate) fn validate_configs(configs: Vec<PathBuf>, printer: Printer) -> Result<ExitStatus> {
    let mut status = ExitStatus::Success;

    if configs.is_empty() {
        warn_user!("No configs to check");
        return Ok(ExitStatus::Success);
    }

    for config_path in configs {
        match read_config(&config_path) {
            Ok(config) => {
                if config.requires_frozen_revs()
                    && write_unfrozen_rev_error(&config, &config_path, printer)?
                {
                    status = ExitStatus::Failure;
                }
            }
            Err(err) => {
                writeln!(printer.stderr(), "{}: {}", "error".red().bold(), err)?;
                for source in iter::successors(err.source(), |&err| err.source()) {
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
            for source in iter::successors(err.source(), |&err| err.source()) {
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
