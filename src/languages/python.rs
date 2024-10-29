use std::path::{Path, PathBuf};
use std::process::Output;

use crate::config;
use crate::hook::Hook;
use crate::languages::LanguageImpl;
use assert_cmd::output::{OutputError, OutputOkExt};
use tokio::process::Command;

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
        let env = hook.environment_dir().unwrap();

        let new_path = std::env::join_paths(
            std::iter::once(bin_dir(env.as_path())).chain(
                std::env::var_os("PATH")
                    .as_ref()
                    .iter()
                    .flat_map(std::env::split_paths),
            ),
        )?;

        // TODO: handle signals
        // TODO: better error display
        let cmds = shlex::split(&hook.entry).ok_or(anyhow::anyhow!("Failed to parse entry"))?;
        let output = Command::new(&cmds[0])
            .args(&cmds[1..])
            .args(&hook.args)
            .args(filenames)
            .env("VIRTUAL_ENV", &env)
            .env("PATH", new_path)
            .env_remove("PYTHONHOME")
            .output()
            .await?;

        Ok(output)
    }
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
