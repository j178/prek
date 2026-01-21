use std::io::Cursor;
use std::path::Path;
use std::process::Stdio;
use std::sync::Arc;

use anyhow::{Context, Result};
use fs_err::tokio as fs;
use tempfile::TempDir;
use tracing::trace;

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

/// Determine if the entry is an inline script or a script path.
/// An entry is considered an inline script if:
/// - It contains newlines (YAML block scalar).
/// - The first token does not resolve to a real file in the repo.
fn is_inline_script(entry: &str, repo_path: &Path) -> bool {
    // YAML block scalar chompping style:
    // |  => keep single trailing newline
    // |- => remove single trailing newline
    // |+ => keep all trailing newlines
    let entry = entry.trim_end_matches(['\n', '\r']);
    if !(entry.contains('\n') || entry.contains('\r')) {
        return false;
    }

    // If we can parse the first token and it resolves to a real file, treat it as a script path.
    // Otherwise, assume the entry is inline script content.
    if let Some(tokens) = shlex::split(entry)
        && let Some(first) = tokens.first()
        && repo_path.join(first).is_file()
    {
        return false;
    }

    true
}

fn parse_inline_shebang(entry: &str) -> Option<Vec<String>> {
    let mut reader = std::io::BufReader::new(Cursor::new(entry.as_bytes()));
    match parse_shebang_from_reader(&mut reader) {
        Ok(cmd) => Some(cmd),
        Err(ShebangError::NoShebang) => None,
        Err(_) => None,
    }
}

#[derive(Debug, Clone)]
struct ShellSpec {
    program: String,
    prefix_args: Vec<String>,
    extension: &'static str,
}

impl ShellSpec {
    fn build_for_script(&self, script_path: &Path) -> Vec<String> {
        let mut cmd = Vec::with_capacity(1 + self.prefix_args.len() + 1);
        cmd.push(self.program.clone());
        cmd.extend(self.prefix_args.iter().cloned());
        cmd.push(script_path.to_string_lossy().to_string());
        cmd
    }
}

#[cfg(not(windows))]
fn resolve_default_shell_spec() -> Result<ShellSpec> {
    let tried = "bash, sh";
    if let Ok(path) = which::which("bash") {
        return Ok(ShellSpec {
            program: path.to_string_lossy().to_string(),
            prefix_args: vec!["-e".to_string()],
            extension: "sh",
        });
    }
    if let Ok(path) = which::which("sh") {
        return Ok(ShellSpec {
            program: path.to_string_lossy().to_string(),
            prefix_args: vec!["-e".to_string()],
            extension: "sh",
        });
    }
    anyhow::bail!("No suitable default shell found (tried {tried})")
}

#[cfg(windows)]
fn resolve_default_shell_spec() -> Result<ShellSpec> {
    let tried = "pwsh, powershell, cmd";
    // Prefer PowerShell 7+ if available.
    if let Ok(path) = which::which("pwsh") {
        return Ok(ShellSpec {
            program: path.to_string_lossy().to_string(),
            prefix_args: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
            ],
            extension: "ps1",
        });
    }
    if let Ok(path) = which::which("powershell") {
        return Ok(ShellSpec {
            program: path.to_string_lossy().to_string(),
            prefix_args: vec![
                "-NoProfile".to_string(),
                "-NonInteractive".to_string(),
                "-ExecutionPolicy".to_string(),
                "Bypass".to_string(),
                "-File".to_string(),
            ],
            extension: "ps1",
        });
    }
    // As a last resort, try cmd.exe.
    if let Ok(path) = which::which("cmd") {
        return Ok(ShellSpec {
            program: path.to_string_lossy().to_string(),
            prefix_args: vec!["/d".to_string(), "/s".to_string(), "/c".to_string()],
            extension: "cmd",
        });
    }

    anyhow::bail!("No suitable default shell found (tried {tried})")
}

fn extension_for_interpreter(interpreter: &str) -> &'static str {
    if interpreter.contains("pwsh") || interpreter.contains("powershell") {
        "ps1"
    } else if interpreter.contains("cmd") {
        "cmd"
    } else if interpreter.contains("python") {
        "py"
    } else {
        "sh"
    }
}

async fn build_inline_entry(
    raw_entry: &str,
    hook_id: &str,
    store: &Store,
) -> Result<(Vec<String>, TempDir)> {
    // If there is a shebang, we can rely on `resolve_command([script_path])` later.
    // If there is no shebang, we choose a reasonable platform-specific default shell.
    let (shebang, default_shell) = if let Some(cmd) = parse_inline_shebang(raw_entry) {
        (Some(cmd), None)
    } else {
        let spec = resolve_default_shell_spec()?;
        trace!(program = %spec.program, "Selected default shell for inline script");
        (None, Some(spec))
    };

    let extension = if let Some(cmd) = &shebang {
        cmd.first()
            .map(|s| extension_for_interpreter(s))
            .unwrap_or("sh")
    } else if let Some(spec) = &default_shell {
        spec.extension
    } else {
        "sh"
    };

    let temp_dir = tempfile::tempdir_in(store.scratch_path())?;
    let script_path = temp_dir
        .path()
        .join(format!("prek-script-{hook_id}.{extension}"));
    fs::write(&script_path, raw_entry)
        .await
        .context("Failed to write inline script")?;

    let entry = if shebang.is_some() {
        // Run the temp script file, honoring its shebang.
        resolve_command(vec![script_path.to_string_lossy().to_string()], None)
    } else {
        // Execute via the chosen default shell by passing the script path.
        let spec = default_shell.expect("default_shell must be set if shebang is None");
        spec.build_for_script(&script_path)
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
        let is_inline = is_inline_script(raw_entry, repo_path);
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
