use std::cmp::max;
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{ExitStatus, Output};
use std::sync::Arc;

use anyhow::Ok;
use assert_cmd::output::{OutputError, OutputOkExt};
use tokio::process::Command;
use tokio::task::JoinSet;

use crate::config;
use crate::hook::Hook;
use crate::languages::LanguageImpl;

#[derive(Debug, Copy, Clone)]
pub struct Python;

impl LanguageImpl for Python {
    fn name(&self) -> config::Language {
        config::Language::Python
    }

    fn default_version(&self) -> &str {
        // TODO find the version of python on the system
        "python3"
    }

    fn environment_dir(&self) -> Option<&str> {
        Some("py_env")
    }

    // TODO: install uv automatically
    // TODO: fallback to pip
    async fn install(&self, hook: &Hook) -> anyhow::Result<()> {
        let venv = hook.environment_dir().expect("No environment dir found");
        // Create venv
        Command::new("uv")
            .arg("venv")
            .arg(&venv)
            .arg("--python")
            .arg(&hook.language_version)
            .output()
            .await
            .map_err(OutputError::with_cause)?
            .ok()?;

        patch_cfg_version_info(&venv).await?;

        // Install dependencies
        Command::new("uv")
            .arg("pip")
            .arg("install")
            .arg(".")
            .args(&hook.additional_dependencies)
            .current_dir(hook.path())
            .env("VIRTUAL_ENV", &venv)
            .output()
            .await
            .map_err(OutputError::with_cause)?
            .ok()?;

        Ok(())
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        todo!()
    }

    async fn run(&self, hook: &Hook, filenames: &[&String]) -> anyhow::Result<Output> {
        // Construct the `PATH` environment variable.
        let env = hook
            .environment_dir()
            .expect("No environment dir for Python");
        let cmds = shlex::split(&hook.entry).ok_or(anyhow::anyhow!("Failed to parse entry"))?;

        let new_path = std::env::join_paths(
            std::iter::once(bin_dir(env.as_path())).chain(
                std::env::var_os("PATH")
                    .as_ref()
                    .iter()
                    .flat_map(std::env::split_paths),
            ),
        )?;

        let concurrency = if hook.require_serial {
            1
        } else {
            // read from config, and the count of cpus
            12
        };
        let partitions = partitions(hook, filenames, concurrency);
        let semaphore = Arc::new(tokio::sync::Semaphore::new(
            concurrency.min(partitions.len()),
        ));

        let mut tasks = JoinSet::new();
        for batch in partitions {
            let semaphore = semaphore.clone();

            let cmds = cmds.clone();
            let hook_args = hook.args.clone();
            let new_path = new_path.clone();
            let env = env.clone();
            let batch = batch
                .into_iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>();

            tasks.spawn(async move {
                let _permit = semaphore
                    .acquire()
                    .await
                    .map_err(|_| anyhow::anyhow!("Semaphore error"))?;

                // TODO: handle signals
                // TODO: better error display
                Command::new(&cmds[0])
                    .args(&cmds[1..])
                    .args(&hook_args)
                    .args(batch)
                    .env("VIRTUAL_ENV", &env)
                    .env("PATH", new_path)
                    .env_remove("PYTHONHOME")
                    .stderr(std::process::Stdio::inherit())
                    .output()
                    .await
                    .map_err(|e| anyhow::anyhow!("Error running command: {:?}", e))
            });
        }

        let mut combined_status = 0;
        let mut combined_stdout = Vec::new();

        while let Some(result) = tasks.join_next().await {
            let output = result??;
            combined_status |= output.status.code().unwrap_or(1);
            combined_stdout = output.stdout;
        }

        Ok(Output {
            status: ExitStatus::from_raw(combined_status),
            stdout: combined_stdout,
            stderr: vec![],
        })
    }
}

fn partitions<'a>(
    hook: &'a Hook,
    filenames: &'a [&String],
    concurrency: usize,
) -> Vec<Vec<&'a String>> {
    let max_per_batch = max(4, filenames.len() / concurrency);
    let max_cli_length = 1 << 12;
    let command_length = hook.entry.len() + hook.args.iter().map(String::len).sum::<usize>();
    // TODO: env size

    let mut partitions = Vec::new();
    let mut current = Vec::new();
    let mut current_length = command_length + 1;

    for &filename in filenames {
        let length = filename.len();
        if current_length + length > max_cli_length || current.len() >= max_per_batch {
            partitions.push(current);
            current = Vec::new();
            current_length = 0;
        }
        current.push(filename);
        current_length += length;
    }

    if !current.is_empty() {
        partitions.push(current);
    }

    partitions
}

fn bin_dir(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts")
    } else {
        venv.join("bin")
    }
}

async fn get_full_version(path: &Path) -> anyhow::Result<String> {
    let python = bin_dir(path).join("python");
    let output = Command::new(&python)
        .arg("-S")
        .arg("-c")
        .arg(r#"import sys; print(".".join(str(p) for p in sys.version_info))"#)
        .output()
        .await
        .map_err(OutputError::with_cause)?
        .ok()?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// Patch pyvenv.cfg `version_info` to ".".join(str(p) for p in sys.version_info)
/// pre-commit use virtualenv to create venv, which sets `version_info` to the full version:
/// "3.12.5.final.0" instead of "3.12.5"
async fn patch_cfg_version_info(path: &Path) -> anyhow::Result<()> {
    let full_version = get_full_version(path).await?;

    let cfg = path.join("pyvenv.cfg");
    let content = fs_err::read_to_string(&cfg)?;
    let mut patched = String::new();
    for line in content.lines() {
        let Some((key, _)) = line.split_once('=') else {
            patched.push_str(line);
            patched.push('\n');
            continue;
        };
        if key.trim() == "version_info" {
            patched.push_str(&format!("version_info = {full_version}\n"));
        } else {
            patched.push_str(line);
            patched.push('\n');
        }
    }

    fs_err::write(&cfg, patched)?;
    Ok(())
}
