use std::fmt::Write;
use std::iter;
use std::path::{Path, PathBuf};

use owo_colors::OwoColorize;

use crate::cli::ExitStatus;
use crate::fs::Simplified;
use crate::printer::Printer;

static SAMPLE_CONFIG: &str = "\
# See https://pre-commit.com for more information
# See https://pre-commit.com/hooks.html for more hooks
repos:
  - repo: 'https://github.com/pre-commit/pre-commit-hooks'
    rev: v5.0.0
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer
      - id: check-yaml
      - id: check-added-large-files
";

#[allow(clippy::print_stdout)]
pub(crate) fn sample_config(file: Option<PathBuf>, printer: Printer) -> ExitStatus {
    if let Some(file) = file {
        if let Err(err) = write_file(&file, SAMPLE_CONFIG, printer) {
            anstream::eprintln!("{}: {}", "error".red().bold(), err);
            for source in iter::successors(err.source(), |&err| err.source()) {
                anstream::eprintln!("  {}: {}", "caused by".red().bold(), source);
            }
            return ExitStatus::Failure;
        }
        return ExitStatus::Success;
    }

    print!("{SAMPLE_CONFIG}");
    ExitStatus::Success
}

fn write_file(file: &Path, content: &str, printer: Printer) -> anyhow::Result<()> {
    fs_err::write(file, content)?;

    writeln!(
        printer.stdout(),
        "Written to `{}`",
        file.simplified_display().cyan()
    )?;
    Ok(())
}
