use std::error::Error;
use std::iter;
use std::path::PathBuf;

use anstream::eprintln;
use owo_colors::OwoColorize;

use crate::cli::ExitStatus;
use crate::config::{read_config, read_manifest};

pub(crate) fn validate_configs(configs: Vec<PathBuf>) -> ExitStatus {
    let mut status = ExitStatus::Success;

    if configs.is_empty() {
        eprintln!("No configs to check");
        return ExitStatus::Success;
    }

    for config in configs {
        if let Err(err) = read_config(&config) {
            eprintln!("{}: {}", "error".red().bold(), err);
            for source in iter::successors(err.source(), |&err| err.source()) {
                eprintln!("  {}: {}", "caused by".red().bold(), source);
            }
            status = ExitStatus::Failure;
        } else {
            eprintln!(
                "{}: {}",
                "success".green().bold(),
                format!("Config `{}` is valid", config.display())
            );
        }
    }

    if status == ExitStatus::Success {
        eprintln!("{}", "All configs are valid".green().bold());
    }

    status
}

pub(crate) fn validate_manifest(manifests: Vec<PathBuf>) -> ExitStatus {
    let mut status = ExitStatus::Success;

    if manifests.is_empty() {
        eprintln!("No manifests to check");
        return ExitStatus::Success;
    }

    for manifest in manifests {
        if let Err(err) = read_manifest(&manifest) {
            eprintln!("{}: {}", "error".red().bold(), err);
            for source in iter::successors(err.source(), |&err| err.source()) {
                eprintln!("  {}: {}", "caused by".red().bold(), source);
            }
            status = ExitStatus::Failure;
        } else {
            eprintln!(
                "{}: {}",
                "success".green().bold(),
                format!("Manifest `{}` is valid", manifest.display())
            );
        }
    }

    if status == ExitStatus::Success {
        eprintln!("{}", "All manifests are valid".green().bold());
    }

    status
}
