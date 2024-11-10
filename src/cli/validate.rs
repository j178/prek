use std::error::Error;
use std::iter;
use std::path::PathBuf;

use anstream::eprintln;
use owo_colors::OwoColorize;

use crate::cli::ExitStatus;
use crate::config::{read_config, read_manifest};

pub(crate) fn validate_configs(configs: Vec<PathBuf>) -> ExitStatus {
    let mut status = ExitStatus::Success;

    for config in configs {
        if let Err(err) = read_config(&config) {
            eprintln!("{}: {}", "error".red().bold(), err);
            for source in iter::successors(err.source(), |&err| err.source()) {
                eprintln!("  {}: {}", "caused by".red().bold(), source);
            }
            status = ExitStatus::Failure;
        }
    }

    status
}

pub(crate) fn validate_manifest(configs: Vec<PathBuf>) -> ExitStatus {
    let mut status = ExitStatus::Success;

    for config in configs {
        if let Err(err) = read_manifest(&config) {
            eprintln!("{}: {}", "error".red().bold(), err);
            for source in iter::successors(err.source(), |&err| err.source()) {
                eprintln!("  {}: {}", "caused by".red().bold(), source);
            }
            status = ExitStatus::Failure;
        }
    }

    status
}
