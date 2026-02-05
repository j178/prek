use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use prek_consts::PREK_TOML;
use toml_edit::{Array, DocumentMut, InlineTable, Value};

use crate::cli::ExitStatus;
use crate::config;
use crate::fs::Simplified;
use crate::printer::Printer;

pub(crate) fn yaml_to_toml(
    input: PathBuf,
    output: Option<PathBuf>,
    force: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    config::load_config(&input).map_err(|err| {
        anyhow::anyhow!("Failed to parse `{}`: {err}", input.simplified_display())
    })?;

    let content = fs_err::read_to_string(&input)
        .with_context(|| format!("Failed to read `{}`", input.simplified_display()))?;
    let value: serde_json::Value = serde_saphyr::from_str(&content).map_err(|err| {
        anyhow::anyhow!(
            "Failed to parse `{}`: {err}",
            input.simplified_display()
        )
    })?;

    let output = output.unwrap_or_else(|| {
        input
            .parent()
            .unwrap_or(Path::new("."))
            .join(PREK_TOML)
    });

    if output == input {
        anyhow::bail!(
            "Output path `{}` matches input; choose a different output path",
            output.simplified_display().cyan()
        );
    }

    let mut rendered = json_to_toml(&value)?;
    if !rendered.ends_with('\n') {
        rendered.push('\n');
    }

    if let Some(parent) = output.parent() {
        fs_err::create_dir_all(parent)?;
    }

    let mut options = fs_err::OpenOptions::new();
    options.write(true);
    if force {
        options.create(true).truncate(true);
    } else {
        options.create_new(true);
    }

    let mut file = match options.open(&output) {
        Ok(file) => file,
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            anyhow::bail!(
                "File `{}` already exists (use --force to overwrite)",
                output.simplified_display().cyan()
            );
        }
        Err(err) => return Err(err.into()),
    };

    file.write_all(rendered.as_bytes())?;

    writeln!(
        printer.stdout(),
        "Written to `{}`",
        output.simplified_display().cyan()
    )?;

    Ok(ExitStatus::Success)
}

fn json_to_toml(value: &serde_json::Value) -> Result<String> {
    let Some(map) = value.as_object() else {
        anyhow::bail!("Expected a top-level mapping in the config file");
    };

    let mut doc = DocumentMut::new();
    for (key, value) in map {
        if value.is_null() {
            continue;
        }
        doc[key] = json_to_toml_value(value);
    }

    Ok(doc.to_string())
}

fn json_to_toml_value(value: &serde_json::Value) -> Value {
    match value {
        serde_json::Value::Null => Value::from(""),
        serde_json::Value::Bool(value) => Value::from(*value),
        serde_json::Value::Number(value) => {
            if let Some(value) = value.as_i64() {
                Value::from(value)
            } else if let Some(value) = value.as_u64() {
                match i64::try_from(value) {
                    Ok(value) => Value::from(value),
                    Err(_) => Value::from(value as f64),
                }
            } else {
                Value::from(value.as_f64().unwrap_or_default())
            }
        }
        serde_json::Value::String(value) => Value::from(value.as_str()),
        serde_json::Value::Array(values) => json_array_to_value(values),
        serde_json::Value::Object(values) => Value::InlineTable(json_object_to_inline(values)),
    }
}

fn json_array_to_value(values: &[serde_json::Value]) -> Value {
    let mut array = Array::new();
    for value in values {
        let value = match value {
            serde_json::Value::Object(map) => Value::InlineTable(json_object_to_inline(map)),
            _ => json_to_toml_value(value),
        };
        array.push(value);
    }
    array.set_trailing_comma(false);
    array.set_trailing_newline(false);
    Value::Array(array)
}

fn json_object_to_inline(values: &serde_json::Map<String, serde_json::Value>) -> InlineTable {
    let mut table = InlineTable::new();
    for (key, value) in values {
        if value.is_null() {
            continue;
        }
        table.insert(key.as_str(), json_to_toml_value(value));
    }
    table
}

#[cfg(test)]
mod tests {
    use super::yaml_to_toml;
    use crate::cli::ExitStatus;
    use crate::printer::Printer;
    use prek_consts::PREK_TOML;

    #[test]
    fn yaml_to_toml_writes_default_path() -> anyhow::Result<()> {
        let temp = tempfile::tempdir()?;
        let input = temp.path().join("config.yaml");

        fs_err::write(
            &input,
            "repos:\n  - repo: local\n    hooks:\n      - id: rustfmt\n",
        )?;

        let status = yaml_to_toml(input, None, false, Printer::Silent)?;
        assert_eq!(status, ExitStatus::Success);

        let output = temp.path().join(PREK_TOML);
        let rendered = fs_err::read_to_string(output)?;
        let value: toml::Value = toml::from_str(&rendered)?;

        let repos = value
            .get("repos")
            .and_then(|repos| repos.as_array())
            .expect("repos array");
        let repo = repos[0]
            .get("repo")
            .and_then(|repo| repo.as_str())
            .expect("repo string");
        assert_eq!(repo, "local");

        let hooks = repos[0]
            .get("hooks")
            .and_then(|hooks| hooks.as_array())
            .expect("hooks array");
        let hook_id = hooks[0]
            .get("id")
            .and_then(|id| id.as_str())
            .expect("hook id");
        assert_eq!(hook_id, "rustfmt");

        Ok(())
    }
}
