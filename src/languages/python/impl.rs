use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::config::LanguageVersion;
use crate::env_vars::EnvVars;
use crate::hook::Hook;
use crate::languages::python::uv::UvInstaller;
use crate::languages::LanguageImpl;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub struct Python;

impl LanguageImpl for Python {
    fn environment_dir(&self) -> Option<&str> {
        Some("py_env")
    }

    // TODO: fallback to virtualenv, pip
    async fn install(&self, hook: &Hook) -> anyhow::Result<()> {
        let venv = hook.environment_dir().expect("No environment dir found");

        let uv = UvInstaller::install().await?;

        let store = Store::from_settings()?;
        let python_install_dir = store.tools_path(ToolBucket::Python);

        let uv_cmd = |summary| {
            #[allow(unused_mut)]
            let mut cmd = Cmd::new(&uv, summary);
            // Don't use cache in Windows, multiple uv instances will conflict with each other.
            // See https://github.com/astral-sh/uv/issues/8664
            #[cfg(windows)]
            cmd.env(EnvVars::UV_NO_CACHE, "1");

            cmd.env(EnvVars::UV_PYTHON_INSTALL_DIR, &python_install_dir);
            cmd
        };

        // Create venv
        let mut cmd = uv_cmd("create venv");
        cmd.arg("venv").arg(&venv);
        match hook.language_version {
            LanguageVersion::Specific(ref version) => {
                cmd.arg("--python").arg(version);
            }
            LanguageVersion::System => {
                cmd.arg("--python-preference").arg("only-system");
            }
            // uv will try to use system Python and download if not found
            LanguageVersion::Default => {}
        }

        cmd.check(true).output().await?;

        // Install dependencies
        uv_cmd("install dependencies")
            .arg("pip")
            .arg("install")
            .arg(".")
            .args(&hook.additional_dependencies)
            .current_dir(hook.path())
            .env("VIRTUAL_ENV", &venv)
            .check(true)
            .output()
            .await?;

        Ok(())
    }

    async fn check_health(&self) -> anyhow::Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: Arc<HashMap<&'static str, String>>,
    ) -> anyhow::Result<(i32, Vec<u8>)> {
        // Get environment directory and parse command
        let env_dir = hook
            .environment_dir()
            .expect("No environment dir for Python");

        let cmds = shlex::split(&hook.entry)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse entry command"))?;

        // Construct PATH with venv bin directory first
        let new_path = std::env::join_paths(
            std::iter::once(bin_dir(env_dir.as_path())).chain(
                std::env::var_os(EnvVars::PATH)
                    .as_ref()
                    .iter()
                    .flat_map(std::env::split_paths),
            ),
        )?;

        let cmds = Arc::new(cmds);
        let hook_args = Arc::new(hook.args.clone());
        let env_dir = Arc::new(env_dir.clone());
        let new_path = Arc::new(new_path);

        let run = move |batch: Vec<String>| {
            // This closure should be Fn, as it is called for each batch. We need to clone the variables,
            // otherwise it will be moved into the async block and can't be used again.
            let cmds = cmds.clone();
            let hook_args = hook_args.clone();
            let env_dir = env_dir.clone();
            let new_path = new_path.clone();
            let env_vars = env_vars.clone();

            // TODO: combine stdout and stderr
            async move {
                let mut output = Cmd::new(&cmds[0], "run python command")
                    .args(&cmds[1..])
                    .env("VIRTUAL_ENV", env_dir.as_ref())
                    .env("PATH", new_path.as_ref())
                    .env_remove("PYTHONHOME")
                    .envs(env_vars.as_ref())
                    .args(hook_args.as_slice())
                    .args(batch)
                    .check(false)
                    .output()
                    .await?;

                output.stdout.extend(output.stderr);
                let code = output.status.code().unwrap_or(1);
                anyhow::Ok((code, output.stdout))
            }
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
