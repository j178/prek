use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use prek_consts::PREK_TOML;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Table, Value};

use crate::cli::ExitStatus;
use crate::config;
use crate::fs::Simplified;
use crate::printer::Printer;

pub(crate) fn yaml_to_toml(
    input: &Path,
    output: Option<PathBuf>,
    force: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    // Validate the input file first.
    config::load_config(input).map_err(|err| {
        anyhow::anyhow!("Failed to parse `{}`: {err}", input.simplified_display())
    })?;

    let content = fs_err::read_to_string(input)?;
    let value: serde_json::Value = serde_saphyr::from_str(&content)?;

    let output = output.unwrap_or_else(|| input.parent().unwrap_or(Path::new(".")).join(PREK_TOML));

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
    let map = value
        .as_object()
        .context("Expected a top-level mapping in the config file")?;

    let mut doc = DocumentMut::new();
    for (key, value) in map {
        if key == "repos" {
            let repos = value.as_array().context("`repos` must be an array")?;
            doc["repos"] = repos_to_array_of_tables(repos)?.into();
            continue;
        }
        doc[key] = json_to_toml_value(value).into();
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
        serde_json::Value::Array(values) => {
            json_array_to_value_with_indent(values, "  ", "  ", false)
        }
        serde_json::Value::Object(values) => Value::InlineTable(json_object_to_inline(values)),
    }
}

fn json_array_to_value_with_indent(
    values: &[serde_json::Value],
    item_indent: &str,
    closing_indent: &str,
    force_multiline: bool,
) -> Value {
    let mut array = Array::new();
    if values.len() == 1 && !force_multiline {
        let value = match &values[0] {
            serde_json::Value::Object(map) => Value::InlineTable(json_object_to_inline(map)),
            _ => json_to_toml_value(&values[0]),
        };
        array.push(value);
        array.set_trailing("");
        return Value::Array(array);
    }

    for value in values {
        let mut value = match value {
            serde_json::Value::Object(map) => Value::InlineTable(json_object_to_inline(map)),
            _ => json_to_toml_value(value),
        };
        value.decor_mut().set_prefix(format!("\n{item_indent}"));
        array.push(value);
    }
    array.set_trailing(format!("\n{closing_indent}"));
    Value::Array(array)
}

fn json_object_to_inline(values: &serde_json::Map<String, serde_json::Value>) -> InlineTable {
    let mut table = InlineTable::new();
    for (key, value) in values {
        let value = match value {
            serde_json::Value::Array(values) => {
                json_array_to_value_with_indent(values, "      ", "    ", false)
            }
            _ => json_to_toml_value(value),
        };
        table.insert(key.as_str(), value);
    }
    format_inline_table_multiline(&mut table);
    table
}

fn format_inline_table_multiline(table: &mut InlineTable) {
    let len = table.len();
    if len <= 1 {
        return;
    }
    for (idx, (mut key, value)) in table.iter_mut().enumerate() {
        key.leaf_decor_mut().set_prefix("\n    ");
        key.leaf_decor_mut().set_suffix(" ");

        let suffix = if idx + 1 == len { "\n  " } else { "" };
        value.decor_mut().set_prefix(" ");
        value.decor_mut().set_suffix(suffix);

        if let Value::InlineTable(inner) = value {
            format_inline_table_multiline(inner);
        }
    }
}

fn repos_to_array_of_tables(values: &[serde_json::Value]) -> Result<ArrayOfTables> {
    let mut array = ArrayOfTables::new();
    for value in values {
        let map = value
            .as_object()
            .context("Each repo entry must be a mapping")?;
        let mut table = Table::new();
        for (key, value) in map {
            if key == "hooks" {
                let hooks = value.as_array().context("`hooks` must be an array")?;
                table[key] = json_array_to_value_with_indent(hooks, "  ", "", true).into();
                continue;
            }
            table[key] = json_to_toml_value(value).into();
        }
        array.push(table);
    }
    Ok(array)
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

        let status = yaml_to_toml(&input, None, false, Printer::Silent)?;
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
