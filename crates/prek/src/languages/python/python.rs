use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use asyncband::once::OnceMap;
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use rustc_hash::FxBuildHasher;
use serde::Deserialize;
use tracing::{debug, trace};

use crate::cli::reporter::HookInstallReporter;
use crate::git::GitCommandExt;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::python::PythonRequest;
use crate::languages::python::uv::Uv;
use crate::languages::version::{LanguageRequest, ToolchainSource};
use crate::languages::{ExecutionEnvironment, LanguageBackend};
use crate::process;
use crate::process::Cmd;
use crate::store::{Store, ToolBucket};

#[derive(Debug, Copy, Clone)]
pub(crate) struct Python;

pub(crate) struct PythonInfo {
    pub(crate) version: semver::Version,
    pub(crate) python_exec: PathBuf,
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PythonInfoError {
    #[error("Failed to parse Python info JSON: {0}")]
    Parse(String),
    #[error("Failed to query Python info: {0}")]
    Query(String),
}

// Canonical paths let virtual environments backed by the same interpreter share one query.
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

    let stdout = Cmd::new(python)
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
    let python = fs_err::canonicalize(python).unwrap_or_else(|_| python.to_path_buf());
    PYTHON_INFO_CACHE
        .try_compute(python.clone(), async move || {
            let info = query_python_info(&python).await?;
            Ok(Arc::new(info))
        })
        .await
}

#[async_trait::async_trait(?Send)]
impl LanguageBackend for Python {
    async fn install(
        &self,
        store: &Store,
        hook: Arc<Hook>,
        install_cwd: &Path,
        reporter: &HookInstallReporter,
    ) -> Result<InstalledHook> {
        let progress = reporter.on_install_start(&hook);

        let uv_dir = store.tools_path(ToolBucket::Uv);
        let uv = Uv::find_or_install(store, &uv_dir)
            .await
            .context("Failed to install uv")?;

        let mut info = InstallInfo::new(&hook, &store.hooks_dir())?;

        debug!(%hook, target = %info.env_path.display(), "Installing environment");

        // Create venv (auto download Python if needed)
        Self::create_venv(&uv, store, &info, &hook.language_request)
            .await
            .context("Failed to create Python virtual environment")?;

        // Install dependencies
        let mut pip_install = Self::pip_install_command(&uv, store, &info.env_path);
        pip_install.current_dir(install_cwd);

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

        let python = python_exec(&info.env_path);
        let python_info = query_python_info(&python)
            .await
            .context("Failed to query Python info")?;

        info.with_language_version(python_info.version)
            .with_toolchain(python_info.python_exec);

        info.persist_env_path();

        reporter.on_install_complete(progress);

        Ok(InstalledHook::Installed {
            hook,
            info: Arc::new(info),
        })
    }

    async fn check_health(&self, info: &InstallInfo) -> Result<()> {
        let python = python_exec(&info.env_path);
        let python_info = query_python_info_cached(&python)
            .await
            .context("Failed to query Python info")?;

        if python_info.version != info.language_version {
            anyhow::bail!(
                "Python version mismatch: expected {}, found {}",
                info.language_version,
                python_info.version
            );
        }

        Ok(())
    }

    fn execution_environment(
        &self,
        _store: &Store,
        hook: &InstalledHook,
    ) -> Result<ExecutionEnvironment> {
        let env_dir = hook.env_path().expect("Python must have env path");
        let new_path = prepend_paths(&[&bin_dir(env_dir)]).context("Failed to join PATH")?;

        let mut environment = ExecutionEnvironment::new();
        environment
            .set_path(&new_path)
            .env(EnvVars::VIRTUAL_ENV, env_dir)
            .env_remove(EnvVars::PYTHONHOME);
        Ok(environment)
    }
}

fn to_uv_python_request(request: &LanguageRequest) -> Option<String> {
    let request: &PythonRequest = request.version();
    match request {
        PythonRequest::Any => None,
        PythonRequest::Major(major) => Some(format!("{major}")),
        PythonRequest::MajorMinor(major, minor) => Some(format!("{major}.{minor}")),
        PythonRequest::MajorMinorPatch(major, minor, patch) => {
            Some(format!("{major}.{minor}.{patch}"))
        }
        PythonRequest::Range(_, raw) => Some(raw.clone()),
    }
}

#[derive(Debug, Clone, Copy)]
enum VenvAttempt {
    PrekManaged,
    External,
    Download,
}

impl Python {
    fn remove_uv_python_override_envs(cmd: &mut Cmd) -> &mut Cmd {
        cmd.env_remove(EnvVars::UV_PYTHON)
            .env_remove(EnvVars::UV_SYSTEM_PYTHON)
    }

    fn pip_install_command(uv: &Uv, store: &Store, env_path: &Path) -> Cmd {
        let mut cmd = uv.cmd(store);
        cmd.arg("pip")
            .arg("install")
            // Explicitly set project to root to avoid uv searching for project-level configs.
            // `--project` has no other effect on `uv pip` subcommands.
            .args(["--project", "/"])
            .env(EnvVars::VIRTUAL_ENV, env_path);
        Self::remove_uv_python_override_envs(&mut cmd)
            // Remove GIT environment variables that may leak from git hooks (e.g., in worktrees).
            // These can break packages using setuptools_scm for file discovery.
            .sanitize_git_repo_env()
            .check(true);
        cmd
    }

    async fn create_venv(
        uv: &Uv,
        store: &Store,
        info: &InstallInfo,
        python_request: &LanguageRequest,
    ) -> Result<()> {
        let policy = python_request.toolchain_policy();
        let mut last_error = None;

        for &source in policy.search_order() {
            let attempt = match source {
                ToolchainSource::Managed => VenvAttempt::PrekManaged,
                ToolchainSource::System => VenvAttempt::External,
            };
            match Self::try_create_venv(uv, store, info, python_request, attempt).await {
                Ok(()) => return Ok(()),
                Err(error @ process::Error::Status { .. }) => {
                    last_error = Some((source, error));
                }
                Err(error) => {
                    debug!(
                        "Failed to create venv `{}`: {error}",
                        info.env_path.display()
                    );
                    return Err(error.into());
                }
            }
        }

        if let Some((ToolchainSource::System, error)) = last_error
            && !Self::can_retry_with_downloads(&error)
        {
            return Err(error.into());
        }

        if policy.allows_download() {
            debug!(
                "Downloading Python into prek's managed store: `{}`",
                info.env_path.display()
            );
            Self::try_create_venv(uv, store, info, python_request, VenvAttempt::Download).await?;
            return Ok(());
        }

        anyhow::bail!("No suitable Python version found for toolchain policy: {policy}")
    }

    async fn try_create_venv(
        uv: &Uv,
        store: &Store,
        info: &InstallInfo,
        python_request: &LanguageRequest,
        attempt: VenvAttempt,
    ) -> std::result::Result<(), process::Error> {
        Self::create_venv_command(uv, store, info, python_request, attempt)
            .check(true)
            .output()
            .await?;
        debug!(
            ?attempt,
            "Created Python virtual environment: `{}`",
            info.env_path.display()
        );
        Ok(())
    }

    fn create_venv_command(
        uv: &Uv,
        store: &Store,
        info: &InstallInfo,
        python_request: &LanguageRequest,
        attempt: VenvAttempt,
    ) -> Cmd {
        let mut cmd = uv.cmd(store);
        cmd.arg("venv").arg(&info.env_path);
        Self::remove_uv_python_override_envs(&mut cmd);

        let python = to_uv_python_request(python_request);
        let mut hidden_args = Vec::from([
            // Avoid discovering a project or workspace.
            "--no-project",
            // Explicitly set project to root to avoid uv searching for project-level configs.
            "--project",
            "/",
        ]);

        match attempt {
            VenvAttempt::PrekManaged | VenvAttempt::Download => {
                // uv maps these variables to `--managed-python` and `--no-managed-python`,
                // which conflict with `--python-preference`.
                cmd.env_remove(EnvVars::UV_MANAGED_PYTHON)
                    .env_remove(EnvVars::UV_NO_MANAGED_PYTHON)
                    .env(
                        EnvVars::UV_PYTHON_INSTALL_DIR,
                        store.tools_path(ToolBucket::Python),
                    );
                hidden_args.extend(["--python-preference", "only-managed"]);
            }
            VenvAttempt::External => {}
        }

        hidden_args.push(match attempt {
            VenvAttempt::Download => "--allow-python-downloads",
            VenvAttempt::PrekManaged | VenvAttempt::External => "--no-python-downloads",
        });

        if let Some(python) = &python {
            hidden_args.extend(["--python", python.as_str()]);
        }
        cmd.hidden_args(hidden_args);

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

    use super::{Python, VenvAttempt};
    use crate::config::Language;
    use crate::hook::InstallInfo;
    use crate::languages::python::uv::Uv;
    use crate::languages::version::LanguageRequest;
    use crate::store::{Store, ToolBucket};
    use prek_consts::env_vars::EnvVars;

    fn setup_test_install() -> (tempfile::TempDir, Uv, Store, InstallInfo) {
        let temp = tempfile::tempdir().expect("create tempdir");
        let hooks_dir = temp.path().join("hooks");
        fs_err::create_dir_all(&hooks_dir).expect("create hooks dir");

        let info = InstallInfo::create(Language::Python, None, Vec::new(), &hooks_dir)
            .expect("create install info");
        let store = Store::from_path(temp.path().join("store")).expect("create store");
        let uv = Uv::new(PathBuf::from("uv"));

        (temp, uv, store, info)
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

    fn assert_venv_attempt(
        attempt: VenvAttempt,
        expected_args: &[&str],
        uses_prek_managed_store: bool,
    ) {
        let (_temp, uv, store, info) = setup_test_install();
        let request = LanguageRequest::parse(Language::Python, "").unwrap();
        let cmd = Python::create_venv_command(&uv, &store, &info, &request, attempt);
        let args = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect::<Vec<_>>();

        assert_eq!(&args[2..], expected_args);
        assert_eq!(
            env_map(&cmd)
                .get(EnvVars::UV_PYTHON_INSTALL_DIR)
                .cloned()
                .flatten(),
            uses_prek_managed_store.then(|| {
                store
                    .tools_path(ToolBucket::Python)
                    .to_string_lossy()
                    .into_owned()
            })
        );
    }

    #[test]
    fn create_venv_command_removes_uv_system_python_override() {
        let (_temp, uv, store, info) = setup_test_install();
        let request = LanguageRequest::parse(Language::Python, "").unwrap();
        let cmd = Python::create_venv_command(&uv, &store, &info, &request, VenvAttempt::External);
        let envs = env_map(&cmd);

        assert_eq!(envs.get(EnvVars::UV_SYSTEM_PYTHON), Some(&None));
        assert_eq!(envs.get(EnvVars::UV_PYTHON), Some(&None));
    }

    #[test]
    fn prek_managed_attempt_uses_only_prek_managed_python() {
        assert_venv_attempt(
            VenvAttempt::PrekManaged,
            &[
                "--no-project",
                "--project",
                "/",
                "--python-preference",
                "only-managed",
                "--no-python-downloads",
            ],
            true,
        );
    }

    #[test]
    fn external_attempt_does_not_override_uv_python_preference() {
        assert_venv_attempt(
            VenvAttempt::External,
            &["--no-project", "--project", "/", "--no-python-downloads"],
            false,
        );
    }

    #[test]
    fn download_attempt_installs_only_into_prek_managed_store() {
        assert_venv_attempt(
            VenvAttempt::Download,
            &[
                "--no-project",
                "--project",
                "/",
                "--python-preference",
                "only-managed",
                "--allow-python-downloads",
            ],
            true,
        );
    }

    #[test]
    fn pip_install_command_removes_uv_system_python_override() {
        let (_temp, uv, store, info) = setup_test_install();
        let cmd = Python::pip_install_command(&uv, &store, &info.env_path);
        let envs = env_map(&cmd);

        assert_eq!(envs.get(EnvVars::UV_SYSTEM_PYTHON), Some(&None));
        assert_eq!(envs.get(EnvVars::UV_PYTHON), Some(&None));
    }
}
