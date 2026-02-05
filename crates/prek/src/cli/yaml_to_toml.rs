use std::fmt::Write as _;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use owo_colors::OwoColorize;
use prek_consts::PREK_TOML;
use toml_edit::{Array, DocumentMut, InlineTable, Value};

use crate::cli::ExitStatus;
use crate::config::{self, FilePattern, HookOptions, Repo};
use crate::fs::Simplified;
use crate::printer::Printer;

pub(crate) fn yaml_to_toml(
    input: PathBuf,
    output: Option<PathBuf>,
    force: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let config = config::load_config(&input).map_err(|err| {
        anyhow::anyhow!("Failed to parse `{}`: {err}", input.simplified_display())
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

    let mut rendered = config_to_toml(&config);
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

fn config_to_toml(config: &config::Config) -> String {
    let mut doc = DocumentMut::new();

    if let Some(value) = &config.minimum_prek_version {
        doc["minimum_prek_version"] = value.as_str().into();
    }
    if let Some(value) = config.orphan {
        doc["orphan"] = value.into();
    }
    if let Some(value) = config.fail_fast {
        doc["fail_fast"] = value.into();
    }
    if let Some(value) = &config.files {
        doc["files"] = file_pattern_to_value(value);
    }
    if let Some(value) = &config.exclude {
        doc["exclude"] = file_pattern_to_value(value);
    }
    if let Some(values) = &config.default_install_hook_types {
        doc["default_install_hook_types"] = inline_array(
            values
                .iter()
                .map(|hook_type| hook_type.to_string())
                .collect::<Vec<_>>(),
        );
    }
    if let Some(values) = &config.default_stages {
        doc["default_stages"] = inline_array(
            values
                .iter()
                .map(|stage| stage.to_string())
                .collect::<Vec<_>>(),
        );
    }
    if let Some(values) = &config.default_language_version {
        let mut table = InlineTable::new();
        let mut entries: Vec<_> = values
            .iter()
            .map(|(lang, version)| (lang.as_ref().to_string(), version))
            .collect();
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        for (lang, version) in entries {
            table.insert(&lang, Value::from(version.as_str()));
        }
        doc["default_language_version"] = Value::InlineTable(table);
    }

    doc["repos"] = repos_to_value(&config.repos);

    doc.to_string()
}

fn repos_to_value(repos: &[Repo]) -> Value {
    let mut array = Array::new();
    for repo in repos {
        array.push(repo_to_inline_table(repo));
    }
    array.set_trailing_comma(false);
    array.set_trailing_newline(false);
    Value::Array(array)
}

fn repo_to_inline_table(repo: &Repo) -> InlineTable {
    let mut table = InlineTable::new();
    match repo {
        Repo::Remote(remote) => {
            table.insert("repo", Value::from(remote.repo.as_str()));
            table.insert("rev", Value::from(remote.rev.as_str()));
            table.insert("hooks", hooks_to_value_remote(&remote.hooks));
        }
        Repo::Local(local) => {
            table.insert("repo", Value::from(local.repo.as_str()));
            table.insert("hooks", hooks_to_value_local(&local.hooks));
        }
        Repo::Meta(meta) => {
            table.insert("repo", Value::from(meta.repo.as_str()));
            table.insert("hooks", hooks_to_value_meta(&meta.hooks));
        }
        Repo::Builtin(builtin) => {
            table.insert("repo", Value::from(builtin.repo.as_str()));
            table.insert("hooks", hooks_to_value_builtin(&builtin.hooks));
        }
    }
    table
}

fn hooks_to_value_remote(hooks: &[config::RemoteHook]) -> Value {
    hooks_to_value(hooks.iter().map(|hook| {
        let mut table = InlineTable::new();
        table.insert("id", Value::from(hook.id.as_str()));
        if let Some(name) = &hook.name {
            table.insert("name", Value::from(name.as_str()));
        }
        if let Some(entry) = &hook.entry {
            table.insert("entry", Value::from(entry.as_str()));
        }
        if let Some(language) = &hook.language {
            table.insert("language", Value::from(language.as_ref()));
        }
        if let Some(priority) = hook.priority {
            table.insert("priority", Value::from(priority));
        }
        add_hook_options(&mut table, &hook.options);
        table
    }))
}

fn hooks_to_value_local(hooks: &[config::LocalHook]) -> Value {
    hooks_to_value(hooks.iter().map(|hook| {
        let mut table = InlineTable::new();
        table.insert("id", Value::from(hook.id.as_str()));
        table.insert("name", Value::from(hook.name.as_str()));
        table.insert("entry", Value::from(hook.entry.as_str()));
        table.insert("language", Value::from(hook.language.as_ref()));
        if let Some(priority) = hook.priority {
            table.insert("priority", Value::from(priority));
        }
        add_hook_options(&mut table, &hook.options);
        table
    }))
}

fn hooks_to_value_meta(hooks: &[config::MetaHook]) -> Value {
    hooks_to_value(hooks.iter().map(|hook| {
        let mut table = InlineTable::new();
        table.insert("id", Value::from(hook.id.as_str()));
        table.insert("name", Value::from(hook.name.as_str()));
        if let Some(priority) = hook.priority {
            table.insert("priority", Value::from(priority));
        }
        add_hook_options(&mut table, &hook.options);
        table
    }))
}

fn hooks_to_value_builtin(hooks: &[config::BuiltinHook]) -> Value {
    hooks_to_value(hooks.iter().map(|hook| {
        let mut table = InlineTable::new();
        table.insert("id", Value::from(hook.id.as_str()));
        table.insert("name", Value::from(hook.name.as_str()));
        table.insert("entry", Value::from(hook.entry.as_str()));
        if let Some(priority) = hook.priority {
            table.insert("priority", Value::from(priority));
        }
        add_hook_options(&mut table, &hook.options);
        table
    }))
}

fn hooks_to_value<I>(hooks: I) -> Value
where
    I: IntoIterator<Item = InlineTable>,
{
    let mut array = Array::new();
    for hook in hooks {
        array.push(hook);
    }
    array.set_trailing_comma(false);
    array.set_trailing_newline(false);
    Value::Array(array)
}

fn add_hook_options(table: &mut InlineTable, options: &HookOptions) {
    if let Some(value) = &options.alias {
        table.insert("alias", Value::from(value.as_str()));
    }
    if let Some(value) = &options.files {
        table.insert("files", file_pattern_to_value(value));
    }
    if let Some(value) = &options.exclude {
        table.insert("exclude", file_pattern_to_value(value));
    }
    if let Some(value) = &options.types {
        table.insert("types", inline_array(value.clone()));
    }
    if let Some(value) = &options.types_or {
        table.insert("types_or", inline_array(value.clone()));
    }
    if let Some(value) = &options.exclude_types {
        table.insert("exclude_types", inline_array(value.clone()));
    }
    if let Some(value) = &options.additional_dependencies {
        table.insert("additional_dependencies", inline_array(value.clone()));
    }
    if let Some(value) = &options.args {
        table.insert("args", inline_array(value.clone()));
    }
    if let Some(value) = &options.env {
        let mut env_table = InlineTable::new();
        let mut entries: Vec<_> = value.iter().collect();
        entries.sort_by(|a, b| a.0.cmp(b.0));
        for (key, value) in entries {
            env_table.insert(key.as_str(), Value::from(value.as_str()));
        }
        table.insert("env", Value::InlineTable(env_table));
    }
    if let Some(value) = options.always_run {
        table.insert("always_run", Value::from(value));
    }
    if let Some(value) = options.fail_fast {
        table.insert("fail_fast", Value::from(value));
    }
    if let Some(value) = options.pass_filenames {
        table.insert("pass_filenames", Value::from(value));
    }
    if let Some(value) = &options.description {
        table.insert("description", Value::from(value.as_str()));
    }
    if let Some(value) = &options.language_version {
        table.insert("language_version", Value::from(value.as_str()));
    }
    if let Some(value) = &options.log_file {
        table.insert("log_file", Value::from(value.as_str()));
    }
    if let Some(value) = options.require_serial {
        table.insert("require_serial", Value::from(value));
    }
    if let Some(value) = &options.stages {
        table.insert(
            "stages",
            inline_array(
                value
                    .iter()
                    .map(|stage| stage.to_string())
                    .collect::<Vec<_>>(),
            ),
        );
    }
    if let Some(value) = options.verbose {
        table.insert("verbose", Value::from(value));
    }
    if let Some(value) = &options.minimum_prek_version {
        table.insert("minimum_prek_version", Value::from(value.as_str()));
    }
}

fn inline_array(values: Vec<String>) -> Value {
    let mut array = Array::new();
    for value in values {
        array.push(value);
    }
    array.set_trailing_comma(false);
    array.set_trailing_newline(false);
    Value::Array(array)
}

fn file_pattern_to_value(pattern: &FilePattern) -> Value {
    if let Some(regex) = pattern.regex_pattern() {
        return Value::from(regex);
    }
    if let Some(globs) = pattern.glob_patterns() {
        let mut table = InlineTable::new();
        if globs.len() == 1 {
            table.insert("glob", Value::from(globs[0].as_str()));
        } else {
            let mut array = Array::new();
            for glob in globs {
                array.push(glob.as_str());
            }
            array.set_trailing_comma(false);
            array.set_trailing_newline(false);
            table.insert("glob", Value::Array(array));
        }
        return Value::InlineTable(table);
    }
    Value::from("")
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
