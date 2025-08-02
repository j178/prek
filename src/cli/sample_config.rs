use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
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
pub(crate) fn sample_config(file: Option<PathBuf>, printer: Printer) -> Result<ExitStatus> {
    if let Some(file) = file {
        fs_err::create_dir_all(file.parent().unwrap_or(Path::new(".")))?;
        if file.exists() {
            anyhow::bail!("File `{}` already exists", file.simplified_display().cyan());
        }
        fs_err::write(&file, SAMPLE_CONFIG)?;

        writeln!(
            printer.stdout(),
            "Written to `{}`",
            file.simplified_display().cyan()
        )?;

        return Ok(ExitStatus::Success);
    }

    print!("{SAMPLE_CONFIG}");
    Ok(ExitStatus::Success)
}
