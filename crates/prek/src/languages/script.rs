use std::io::Cursor;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use fs_err::tokio as fs;
use tempfile::TempDir;

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::identify::{ShebangError, parse_shebang_from_reader};
use crate::languages::{LanguageImpl, resolve_command};
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::Store;

#[derive(Debug, Copy, Clone)]
pub(crate) struct Script;

fn is_script_path_entry(entry: &str, repo_path: &Path) -> bool {
    // Ignore trailing newlines so a path with a trailing line break isn't treated as inline.
    let trimmed = entry.trim_end_matches(['\n', '\r']);
    if !(trimmed.contains('\n') || trimmed.contains('\r')) {
        return true;
    }

    // If we can parse the first token and it resolves to a real file, treat it as a script path.
    // Otherwise, assume the entry is inline script content.
    let Some(tokens) = shlex::split(trimmed) else {
        return false;
    };
    let Some(first) = tokens.first() else {
        return false;
    };

    let candidate = repo_path.join(first);
    candidate.is_file()
}

fn parse_inline_shebang(entry: &str) -> Option<Vec<String>> {
    let mut reader = std::io::BufReader::new(Cursor::new(entry.as_bytes()));
    match parse_shebang_from_reader(&mut reader) {
        Ok(cmd) => Some(cmd),
        Err(ShebangError::NoShebang) => None,
        Err(_) => None,
    }
}

fn inline_extension_from_interpreter(interpreter: Option<&str>) -> &'static str {
    match interpreter.map(str::to_ascii_lowercase) {
        Some(value) if value.contains("pwsh") || value.contains("powershell") => "ps1",
        _ => "sh",
    }
}

fn resolve_default_shell() -> Result<std::path::PathBuf> {
    if let Ok(path) = which::which("bash") {
        return Ok(path);
    }
    if let Ok(path) = which::which("sh") {
        return Ok(path);
    }
    anyhow::bail!("Inline script requires `bash` or `sh` in PATH")
}

fn inline_shebang_command(script_path: &Path, mut cmd: Vec<String>) -> Vec<String> {
    let interpreter = cmd
        .first()
        .map(|value| value.to_ascii_lowercase())
        .unwrap_or_default();
    if interpreter.contains("pwsh") || interpreter.contains("powershell") {
        cmd.push("-NoProfile".to_string());
        cmd.push("-NonInteractive".to_string());
        cmd.push("-File".to_string());
        cmd.push(script_path.to_string_lossy().to_string());
        return cmd;
    }

    cmd.push(script_path.to_string_lossy().to_string());
    cmd
}

async fn build_inline_entry(
    raw_entry: &str,
    hook_id: &str,
    store: &Store,
) -> Result<(Vec<String>, TempDir)> {
    // Parse the shebang from the inline content (if any) to choose interpreter + extension.
    let shebang = parse_inline_shebang(raw_entry);
    let extension = inline_extension_from_interpreter(
        shebang
            .as_ref()
            .and_then(|cmd| cmd.first())
            .map(String::as_str),
    );

    let temp_dir = tempfile::tempdir_in(store.scratch_path())?;
    let script_path = temp_dir
        .path()
        .join(format!("prek-script-{hook_id}.{extension}"));
    fs::write(&script_path, raw_entry)
        .await
        .context("Failed to write inline script")?;

    // Build the command line using the shebang if present; otherwise use bash/sh.
    let entry = if let Some(cmd) = shebang {
        let entry = inline_shebang_command(&script_path, cmd);
        resolve_command(entry, None)
    } else {
        let shell = resolve_default_shell()?;
        vec![
            shell.to_string_lossy().to_string(),
            script_path.to_string_lossy().to_string(),
        ]
    };

    Ok((entry, temp_dir))
}

impl LanguageImpl for Script {
    async fn install(
        &self,
        hook: Arc<Hook>,
        _store: &Store,
        _reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self, _info: &InstallInfo) -> Result<()> {
        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        // For `language: script`, the `entry[0]` is a script path.
        // For remote hooks, the path is relative to the repo root.
        // For local hooks, the path is relative to the current working directory.
        // If the entry is an inline script, we write it to a temp file and run it.

        let progress = reporter.on_run_start(hook, filenames.len());

        let raw_entry = hook.entry.raw();
        let repo_path = hook.repo_path().unwrap_or(hook.work_dir());
        let is_inline = !is_script_path_entry(raw_entry, repo_path);
        let mut inline_temp: Option<TempDir> = None;

        let entry = if is_inline {
            let (entry, temp_dir) = build_inline_entry(raw_entry, &hook.id, store).await?;
            inline_temp = Some(temp_dir);
            entry
        } else {
            let mut split = hook.entry.split()?;

            let cmd = repo_path.join(&split[0]);
            split[0] = cmd.to_string_lossy().to_string();
            resolve_command(split, None)
        };

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "run script command")
                .current_dir(hook.work_dir())
                .envs(&hook.env)
                .args(&entry[1..])
                .args(&hook.args)
                .args(batch)
                .check(false)
                .stdin(Stdio::null())
                .pty_output()
                .await?;

            reporter.on_run_progress(progress, batch.len() as u64);

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, &entry, run).await?;
        drop(inline_temp);

        reporter.on_run_complete(progress);

        // Collect results
        let mut combined_status = 0;
        let mut combined_output = Vec::new();

        for (code, output) in results {
            combined_status |= code;
            combined_output.extend(output);
        }

        Ok((combined_status, combined_output))
    }
}
