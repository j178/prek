use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use tracing::debug;

use crate::hook::ResolvedHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::LanguageImpl;
use crate::languages::python::uv::Uv;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{Store, ToolBucket};

use constants::env_vars::EnvVars;

#[derive(Debug, Copy, Clone)]
pub struct Python;

impl LanguageImpl for Python {
    fn supports_dependency(&self) -> bool {
        true
    }

    async fn resolve(&self, hook: &Hook, store: &Store) -> Result<ResolvedHook> {
        // Select from installed hooks
        if let Some(info) = store.installed_hooks().find(|info| info.matches(hook)) {
            debug!(
                "Found installed environment for {}: {}",
                hook,
                info.env_path.display()
            );
            return Ok(ResolvedHook::Installed {
                hook: hook.clone(),
                info,
            });
        }
        debug!("No matching installed environment found for {}", hook);

        // Select toolchain from system or managed
        let uv = Uv::install(store).await?;
        let python = uv
            .find_python(hook, store)
            .await?
            .into_iter()
            .next()
            .ok_or_else(|| anyhow!("Failed to resolve hook"))?;
        debug!(python = %python.display(), "Resolved Python");

        // Get Python version
        let stdout = Cmd::new(&python, "get Python version")
            .arg("-I")
            .arg("-c")
            .arg("import sys; print('.'.join(map(str, sys.version_info[:3])))")
            .check(true)
            .output()
            .await?
            .stdout;
        let version = String::from_utf8_lossy(&stdout)
            .trim()
            .parse::<semver::Version>()
            .with_context(|| "Failed to parse Python version")?;

        Ok(ResolvedHook::NotInstalled {
            hook: hook.clone(),
            toolchain: python.clone(),
            info: InstallInfo::new(hook.language, version, hook.dependencies().to_vec(), store),
        })
    }

    async fn install(&self, hook: &ResolvedHook, store: &Store) -> Result<()> {
        let ResolvedHook::NotInstalled {
            hook,
            toolchain,
            info,
        } = hook
        else {
            unreachable!("Python hook must be NotInstalled")
        };

        let uv = Uv::install(store).await?;

        // Create venv
        let mut cmd = uv.cmd("create venv");
        cmd.arg("venv")
            .arg(&info.env_path)
            .arg("--python")
            .arg(toolchain)
            .env(
                EnvVars::UV_PYTHON_INSTALL_DIR,
                store.tools_path(ToolBucket::Python),
            );

        cmd.check(true).output().await?;

        // Install dependencies
        if let Some(repo_path) = hook.repo_path() {
            uv.cmd("install dependencies")
                .arg("pip")
                .arg("install")
                .arg(".")
                .args(&hook.additional_dependencies)
                .current_dir(repo_path)
                .env("VIRTUAL_ENV", &info.env_path)
                .check(true)
                .output()
                .await?;
        } else if !hook.additional_dependencies.is_empty() {
            uv.cmd("install dependencies")
                .arg("pip")
                .arg("install")
                .args(&hook.additional_dependencies)
                .env("VIRTUAL_ENV", &info.env_path)
                .check(true)
                .output()
                .await?;
        } else {
            debug!("No dependencies to install");
        }
        Ok(())
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &ResolvedHook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        // Get environment directory and parse command
        let env_dir = hook.env_path().expect("Python must have env path");

        let cmds = shlex::split(&hook.entry)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse entry command"))?;

        // Construct PATH with venv bin directory first
        let new_path = std::env::join_paths(
            std::iter::once(bin_dir(env_dir)).chain(
                EnvVars::var_os(EnvVars::PATH)
                    .as_ref()
                    .iter()
                    .flat_map(std::env::split_paths),
            ),
        )?;

        let run = async move |batch: Vec<String>| {
            // TODO: combine stdout and stderr
            let mut output = Cmd::new(&cmds[0], "run python command")
                .args(&cmds[1..])
                .env("VIRTUAL_ENV", env_dir)
                .env("PATH", &new_path)
                .env_remove("PYTHONHOME")
                .envs(env_vars)
                .args(&hook.args)
                .args(batch)
                .check(false)
                .output()
                .await?;

            output.stdout.extend(output.stderr);
            let code = output.status.code().unwrap_or(1);
            anyhow::Ok((code, output.stdout))
        };

        let results = run_by_batch(hook, filenames, run).await?;

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

fn bin_dir(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts")
    } else {
        venv.join("bin")
    }
}
