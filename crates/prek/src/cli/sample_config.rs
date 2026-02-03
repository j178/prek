use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use owo_colors::OwoColorize;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PREK_TOML};

use crate::cli::{ExitStatus, SampleConfigFormat, SampleConfigTarget};
use crate::fs::Simplified;
use crate::printer::Printer;

static SAMPLE_CONFIG_YAML: &str = indoc::indoc! {"
# See https://pre-commit.com for more information
# See https://pre-commit.com/hooks.html for more hooks
repos:
  - repo: 'https://github.com/pre-commit/pre-commit-hooks'
    rev: v6.0.0
    hooks:
      - id: trailing-whitespace
      - id: end-of-file-fixer
      - id: check-yaml
      - id: check-added-large-files
"};

static SAMPLE_CONFIG_TOML: &str = indoc::indoc! {r#"
#:schema: https://prek.j178.dev/prek.schema.json
# Configuration file for `prek`, a git hook framework written in Rust.
# See https://prek.j178.dev for more information.

[[repos]]
repo = "builtin"
hooks = [
    { id = "trailing-whitespace" },
    { id = "end-of-file-fixer" },
    { id = "check-added-large-files" },
]"#};

pub(crate) fn sample_config(
    target: SampleConfigTarget,
    format: Option<SampleConfigFormat>,
    printer: Printer,
) -> Result<ExitStatus> {
    let (path, format) = match (target, format) {
        (SampleConfigTarget::Path(path), Some(format)) => (Some(path), format),
        (SampleConfigTarget::Path(path), None) => match path.extension() {
            Some(ext) if ext.eq_ignore_ascii_case("toml") => (Some(path), SampleConfigFormat::Toml),
            _ => (Some(path), SampleConfigFormat::Yaml),
        },
        (SampleConfigTarget::DefaultFile, Some(format)) => match format {
            SampleConfigFormat::Toml => (Some(PathBuf::from(PREK_TOML)), format),
            SampleConfigFormat::Yaml => (Some(PathBuf::from(PRE_COMMIT_CONFIG_YAML)), format),
        },
        (SampleConfigTarget::DefaultFile, None) => (
            Some(PathBuf::from(PRE_COMMIT_CONFIG_YAML)),
            SampleConfigFormat::Yaml,
        ),
        (SampleConfigTarget::Stdout, Some(format)) => (None, format),
        (SampleConfigTarget::Stdout, None) => (None, SampleConfigFormat::Yaml),
    };

    if let Some(path) = path {
        fs_err::create_dir_all(path.parent().unwrap_or(Path::new(".")))?;
        let mut file = match fs_err::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(f) => f,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                anyhow::bail!("File `{}` already exists", path.simplified_display().cyan());
            }
            Err(err) => return Err(err.into()),
        };

        match format {
            SampleConfigFormat::Yaml => write!(file, "{SAMPLE_CONFIG_YAML}")?,
            SampleConfigFormat::Toml => write!(file, "{SAMPLE_CONFIG_TOML}")?,
        }

        writeln!(
            printer.stdout(),
            "Written to `{}`",
            path.simplified_display().cyan()
        )?;

        return Ok(ExitStatus::Success);
    }

    // TODO: default to prek.toml in the future?
    match format {
        SampleConfigFormat::Yaml => {
            writeln!(printer.stdout_important(), "{SAMPLE_CONFIG_YAML}")?;
        }
        SampleConfigFormat::Toml => {
            writeln!(printer.stdout_important(), "{SAMPLE_CONFIG_TOML}")?;
        }
    }
    Ok(ExitStatus::Success)
}
