use std::env::consts::EXE_EXTENSION;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use mea::once::OnceMap;
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use rustc_hash::FxBuildHasher;
use serde::Deserialize;
use tracing::{debug, trace};

use crate::cli::reporter::{HookInstallReporter, HookRunReporter};
use crate::config::PythonUvLockMode;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstalledHookEnv};
use crate::languages::LanguageImpl;
use crate::languages::python::PythonRequest;
use crate::languages::python::uv::Uv;
use crate::languages::version::LanguageRequest;
use crate::process;
use crate::process::Cmd;
use crate::run::run_by_batch;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Python;

#[derive(Debug, Copy, Clone)]
pub(crate) struct PythonUv;

pub(crate) struct PythonInfo {
    pub(crate) version: semver::Version,
    pub(crate) python_exec: PathBuf,
}

#[derive(Debug, Clone, thiserror::Error)]
pub(crate) enum PythonInfoError {
    #[error("Failed to parse Python info JSON: {0}")]
    Parse(String),
    #[error("Failed to query Python info: {0}")]
    Query(String),
    #[error("{0}")]
    Message(String),
}

static PYTHON_INFO_CACHE: LazyLock<OnceMap<PathBuf, Arc<PythonInfo>, FxBuildHasher>> =
    LazyLock::new(|| OnceMap::with_hasher(FxBuildHasher));

async fn query_python_info(python: &Path) -> Result<PythonInfo, PythonInfoError> {
    #[derive(Deserialize)]
    struct QueryPythonInfo {
        version: semver::Version,
        base_exec_prefix: PathBuf,
    }

    static QUERY_PYTHON_INFO: &str = indoc::indoc! {r#"
    import sys, json
    info = {
        "version": ".".join(map(str, sys.version_info[:3])),
        "base_exec_prefix": sys.base_exec_prefix,
    }
    print(json.dumps(info))
    "#};

    let stdout = Cmd::new(python, "python -c")
        .arg("-I")
        .arg("-c")
        .arg(QUERY_PYTHON_INFO)
        .check(true)
        .output()
        .await
        .map_err(|err| PythonInfoError::Query(err.to_string()))?
        .stdout;

    let info: QueryPythonInfo =
        serde_json::from_slice(&stdout).map_err(|err| PythonInfoError::Parse(err.to_string()))?;
    let python_exec = python_exec(&info.base_exec_prefix);

    Ok(PythonInfo {
        version: info.version,
        python_exec,
    })
}

pub(crate) async fn query_python_info_cached(
    python: &Path,
) -> Result<Arc<PythonInfo>, PythonInfoError> {
    let python = fs::canonicalize(python).unwrap_or_else(|_| python.to_path_buf());
    PYTHON_INFO_CACHE
        .try_compute(python.clone(), async move || {
            let info = query_python_info(&python).await?;
            Ok(Arc::new(info))
        })
        .await
}

impl LanguageImpl for Python {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let uv_dir = store.tools_path(ToolBucket::Uv);
        let uv = Uv::install(store, &uv_dir)
            .await
            .context("Failed to install uv")?;

        let mut env = InstalledHookEnv::new(
            hook.language,
            hook.env_identity().into(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %env.env_path.display(), "Installing environment");

        // Create venv (auto download Python if needed)
        Self::create_venv(&uv, store, &env, &hook.language_request)
            .await
            .context("Failed to create Python virtual environment")?;

        // Install dependencies
        let mut pip_install = Self::pip_install_command(&uv, store, &env.env_path);

        if let Some(repo_path) = hook.repo_path() {
            trace!(
                "Installing dependencies from repo path: {}",
                repo_path.display()
            );
            pip_install
                .arg("--directory")
                .arg(repo_path)
                .arg(".")
                .args(&hook.additional_dependencies)
                .output()
                .await?;
        } else if !hook.additional_dependencies.is_empty() {
            trace!(
                "Installing additional dependencies: {:?}",
                hook.additional_dependencies
            );
            pip_install
                .args(&hook.additional_dependencies)
                .output()
                .await?;
        } else {
            debug!("No dependencies to install");
        }

        let python = python_exec(&env.env_path);
        let python_info = query_python_info(&python)
            .await
            .context("Failed to query Python info")?;

        env.with_language_version(python_info.version)
            .with_toolchain(python_info.python_exec);

        env.persist();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            env: Arc::new(env),
        })
    }

    async fn check_health(&self, env: &InstalledHookEnv) -> Result<()> {
        let python = python_exec(&env.env_path);
        let python_info = query_python_info_cached(&python)
            .await
            .context("Failed to query Python info")?;

        if python_info.version != env.language_version {
            anyhow::bail!(
                "Python version mismatch: expected {}, found {}",
                env.language_version,
                python_info.version
            );
        }

        Ok(())
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        let progress = reporter.on_run_start(hook, filenames.len());

        let env_dir = hook.env_path().expect("Python must have env path");
        let new_path = prepend_paths(&[&bin_dir(env_dir)]).context("Failed to join PATH")?;
        let entry = hook.entry.resolve(Some(&new_path), store)?;

        let run = async |batch: &[&Path]| {
            let mut output = Cmd::new(&entry[0], "python hook")
                .current_dir(hook.work_dir())
                .args(&entry[1..])
                .env(EnvVars::VIRTUAL_ENV, env_dir)
                .env(EnvVars::PATH, &new_path)
                .env_remove(EnvVars::PYTHONHOME)
                .envs(&hook.env)
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

        let results = run_by_batch(hook, filenames, entry.argv(), run).await?;

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

impl LanguageImpl for PythonUv {
    async fn install(
        &self,
        hook: Arc<Hook>,
        store: &Store,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let uv_dir = store.tools_path(ToolBucket::Uv);
        let uv = Uv::install(store, &uv_dir)
            .await
            .context("Failed to install uv")?;

        let mut env = InstalledHookEnv::new(
            hook.language,
            hook.env_identity().into(),
            &store.hooks_dir(),
        )?;

        debug!(%hook, target = %env.env_path.display(), "Installing python_uv environment");

        Python::create_venv(&uv, store, &env, &hook.language_request)
            .await
            .context("Failed to create Python virtual environment")?;

        let uv_env = hook
            .python_uv_env()
            .expect("python_uv hook must have uv env options");
        Python::sync_project_environment_command(&uv, store, &env.env_path, uv_env)
            .output()
            .await
            .context("Failed to sync uv project environment")?;

        let python = python_exec(&env.env_path);
        let python_info = query_python_info(&python)
            .await
            .context("Failed to query Python info")?;

        env.with_language_version(python_info.version)
            .with_toolchain(python_info.python_exec);

        env.persist();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            env: Arc::new(env),
        })
    }

    async fn check_health(&self, env: &InstalledHookEnv) -> Result<()> {
        Python.check_health(env).await
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&Path],
        store: &Store,
        reporter: &HookRunReporter,
    ) -> Result<(i32, Vec<u8>)> {
        Python.run(hook, filenames, store, reporter).await
    }
}

fn to_uv_python_request(request: &LanguageRequest) -> Option<String> {
    match request {
        LanguageRequest::Any { .. } => None,
        LanguageRequest::Python(request) => match request {
            PythonRequest::Any => None,
            PythonRequest::Major(major) => Some(format!("{major}")),
            PythonRequest::MajorMinor(major, minor) => Some(format!("{major}.{minor}")),
            PythonRequest::MajorMinorPatch(major, minor, patch) => {
                Some(format!("{major}.{minor}.{patch}"))
            }
            PythonRequest::Range(_, raw) => Some(raw.clone()),
            PythonRequest::Path(path) => Some(path.to_string_lossy().to_string()),
        },
        _ => unreachable!(),
    }
}

impl Python {
    fn remove_uv_python_override_envs(cmd: &mut Cmd) -> &mut Cmd {
        // Ensure uv selects the hook virtualenv interpreter.
        cmd.env_remove(EnvVars::UV_PYTHON)
            .env_remove(EnvVars::UV_SYSTEM_PYTHON)
            // `--managed-python` and `--no-managed-python` conflict with our explicit preference.
            .env_remove(EnvVars::UV_MANAGED_PYTHON)
            .env_remove(EnvVars::UV_NO_MANAGED_PYTHON)
    }

    fn pip_install_command(uv: &Uv, store: &Store, env_path: &Path) -> Cmd {
        let mut cmd = uv.cmd("uv pip", store);
        cmd.arg("pip")
            .arg("install")
            // Explicitly set project to root to avoid uv searching for project-level configs.
            // `--project` has no other effect on `uv pip` subcommands.
            .args(["--project", "/"])
            .env(EnvVars::VIRTUAL_ENV, env_path);
        Self::remove_uv_python_override_envs(&mut cmd)
            // Remove GIT environment variables that may leak from git hooks (e.g., in worktrees).
            // These can break packages using setuptools_scm for file discovery.
            .remove_git_envs()
            .check(true);
        cmd
    }

    fn sync_project_environment_command(
        uv: &Uv,
        store: &Store,
        env_path: &Path,
        uv_env: &crate::hook_env::PythonUvEnv,
    ) -> Cmd {
        let mut cmd = uv.cmd("uv sync", store);
        cmd.arg("sync")
            .arg("--project")
            .arg(&uv_env.project)
            .arg("--active")
            .arg("--no-default-groups")
            .env(EnvVars::VIRTUAL_ENV, env_path)
            .current_dir(&uv_env.project);

        match uv_env.lock_mode {
            PythonUvLockMode::Locked => {
                cmd.arg("--locked");
            }
            PythonUvLockMode::Frozen => {
                cmd.arg("--frozen");
            }
        }

        for group in &uv_env.dependency_groups {
            cmd.arg("--group").arg(group);
        }
        for extra in &uv_env.extras {
            cmd.arg("--extra").arg(extra);
        }
        if !uv_env.install_project {
            cmd.arg("--no-install-project");
        }

        Self::remove_uv_python_override_envs(&mut cmd)
            .remove_git_envs()
            .check(true);
        cmd
    }

    async fn create_venv(
        uv: &Uv,
        store: &Store,
        env: &InstalledHookEnv,
        python_request: &LanguageRequest,
    ) -> Result<()> {
        // Try creating venv without downloads first
        match Self::create_venv_command(uv, store, env, python_request, false, false)
            .check(true)
            .output()
            .await
        {
            Ok(_) => {
                debug!(
                    "Venv created successfully with no downloads: `{}`",
                    env.env_path.display()
                );
                Ok(())
            }
            Err(e @ process::Error::Status { .. }) => {
                // Check if we can retry with downloads
                if Self::can_retry_with_downloads(&e) {
                    if !python_request.allows_download() {
                        anyhow::bail!(
                            "No suitable system Python version found and downloads are disabled"
                        );
                    }

                    debug!(
                        "Retrying venv creation with managed Python downloads: `{}`",
                        env.env_path.display()
                    );
                    Self::create_venv_command(uv, store, env, python_request, true, true)
                        .check(true)
                        .output()
                        .await?;
                    return Ok(());
                }
                // If we can't retry, return the original error
                Err(e.into())
            }
            Err(e) => {
                debug!("Failed to create venv `{}`: {e}", env.env_path.display());
                Err(e.into())
            }
        }
    }

    fn create_venv_command(
        uv: &Uv,
        store: &Store,
        env: &InstalledHookEnv,
        python_request: &LanguageRequest,
        set_install_dir: bool,
        allow_downloads: bool,
    ) -> Cmd {
        let mut cmd = uv.cmd("create venv", store);
        cmd.arg("venv")
            .arg(&env.env_path)
            .args(["--python-preference", "managed"])
            // Avoid discovering a project or workspace
            .arg("--no-project")
            // Explicitly set project to root to avoid uv searching for project-level configs
            .args(["--project", "/"]);
        Self::remove_uv_python_override_envs(&mut cmd);
        if set_install_dir {
            cmd.env(
                EnvVars::UV_PYTHON_INSTALL_DIR,
                store.tools_path(ToolBucket::Python),
            );
        }
        if allow_downloads {
            cmd.arg("--allow-python-downloads");
        } else {
            cmd.arg("--no-python-downloads");
        }

        if let Some(python) = to_uv_python_request(python_request) {
            cmd.arg("--python").arg(python);
        }

        cmd
    }

    fn can_retry_with_downloads(error: &process::Error) -> bool {
        let process::Error::Status {
            error:
                process::StatusError {
                    output: Some(output),
                    ..
                },
            ..
        } = error
        else {
            return false;
        };

        let stderr = String::from_utf8_lossy(&output.stderr);
        stderr.contains("A managed Python download is available")
    }
}

fn bin_dir(venv: &Path) -> PathBuf {
    if cfg!(windows) {
        venv.join("Scripts")
    } else {
        venv.join("bin")
    }
}

pub(crate) fn python_exec(venv: &Path) -> PathBuf {
    bin_dir(venv).join("python").with_extension(EXE_EXTENSION)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::path::PathBuf;

    use prek_consts::env_vars::EnvVars;

    use super::Python;
    use crate::config::{Language, PythonUvLockMode, PythonUvOptions};
    use crate::hook::InstalledHookEnv;
    use crate::hook_env::PythonUvEnv;
    use crate::languages::python::uv::Uv;
    use crate::languages::version::LanguageRequest;
    use crate::store::Store;

    fn setup_test_install() -> (tempfile::TempDir, Uv, Store, InstalledHookEnv) {
        let temp = tempfile::tempdir().expect("create tempdir");
        let hooks_dir = temp.path().join("hooks");
        fs_err::create_dir_all(&hooks_dir).expect("create hooks dir");

        let env = InstalledHookEnv::new(
            Language::Python,
            crate::hook_env::HookEnvIdentity::empty_dependencies(),
            &hooks_dir,
        )
        .expect("create installed hook environment");
        let store = Store::from_path(temp.path().join("store"));
        let uv = Uv::new(PathBuf::from("uv"));

        (temp, uv, store, env)
    }

    fn env_map(cmd: &crate::process::Cmd) -> HashMap<String, Option<String>> {
        cmd.get_envs()
            .map(|(key, val)| {
                (
                    key.to_string_lossy().into_owned(),
                    val.map(|v| v.to_string_lossy().into_owned()),
                )
            })
            .collect()
    }

    fn args(cmd: &crate::process::Cmd) -> Vec<String> {
        cmd.get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn create_venv_command_removes_uv_system_python_override() {
        let (_temp, uv, store, env) = setup_test_install();
        let request = LanguageRequest::Any { system_only: false };
        let cmd = Python::create_venv_command(&uv, &store, &env, &request, false, false);
        let envs = env_map(&cmd);

        assert_eq!(envs.get(EnvVars::UV_SYSTEM_PYTHON), Some(&None));
        assert_eq!(envs.get(EnvVars::UV_PYTHON), Some(&None));
        assert_eq!(envs.get(EnvVars::UV_MANAGED_PYTHON), Some(&None));
        assert_eq!(envs.get(EnvVars::UV_NO_MANAGED_PYTHON), Some(&None));
    }

    #[test]
    fn pip_install_command_removes_uv_system_python_override() {
        let (_temp, uv, store, env) = setup_test_install();
        let cmd = Python::pip_install_command(&uv, &store, &env.env_path);
        let envs = env_map(&cmd);

        assert_eq!(envs.get(EnvVars::UV_SYSTEM_PYTHON), Some(&None));
        assert_eq!(envs.get(EnvVars::UV_PYTHON), Some(&None));
        assert_eq!(envs.get(EnvVars::UV_MANAGED_PYTHON), Some(&None));
        assert_eq!(envs.get(EnvVars::UV_NO_MANAGED_PYTHON), Some(&None));
    }

    #[test]
    fn sync_project_environment_command_targets_managed_venv() {
        let temp = tempfile::tempdir().expect("create tempdir");
        fs_err::write(
            temp.path().join("pyproject.toml"),
            "[project]\nname = \"example\"\nversion = \"0.1.0\"\n",
        )
        .expect("write pyproject");
        fs_err::write(temp.path().join("uv.lock"), "version = 1\n").expect("write lockfile");

        let (uv_env, _) = PythonUvEnv::resolve(
            &PythonUvOptions {
                dependency_groups: Some(vec!["typecheck".to_string()]),
                extras: Some(vec!["typed".to_string()]),
                install_project: Some(false),
                lock_mode: Some(PythonUvLockMode::Frozen),
                ..Default::default()
            },
            temp.path(),
        )
        .expect("resolve python_uv env");

        let uv = Uv::new(PathBuf::from("uv"));
        let store = Store::from_path(temp.path().join("store"));
        let env_path = temp.path().join("managed-venv");
        let cmd = Python::sync_project_environment_command(&uv, &store, &env_path, &uv_env);

        assert_eq!(
            args(&cmd),
            [
                "sync",
                "--project",
                temp.path().canonicalize().unwrap().to_str().unwrap(),
                "--active",
                "--no-default-groups",
                "--frozen",
                "--group",
                "typecheck",
                "--extra",
                "typed",
                "--no-install-project",
            ]
        );

        let envs = env_map(&cmd);
        assert_eq!(
            envs.get(EnvVars::VIRTUAL_ENV),
            Some(&Some(env_path.to_string_lossy().into_owned()))
        );
        assert_eq!(envs.get(EnvVars::UV_SYSTEM_PYTHON), Some(&None));
        assert_eq!(cmd.get_current_dir(), Some(uv_env.project.as_path()));
    }
}
