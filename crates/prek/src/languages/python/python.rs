use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use mea::once::OnceMap;
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use regex::Regex;
use rustc_hash::FxBuildHasher;
use serde::Deserialize;
use tracing::debug;

use crate::cli::reporter::HookInstallReporter;
use crate::git::GitCommandExt;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::python::PythonRequest;
use crate::languages::python::uv::Uv;
use crate::languages::version::LanguageRequest;
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
        Self::install_dependencies(&uv, store, &info, &hook, &hook.language_request).await?;

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

/// The highest `requires-python` lower bound in a uv resolution failure, patch included so the
/// compatibility check against the hook's original request stays sound (e.g. against `<3.11.2`).
fn infer_python_request(stderr: &[u8]) -> Option<semver::Version> {
    static PYTHON_BOUND: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"Python\s*>=?\s*(\d+\.\d+(?:\.\d+)?)").unwrap());

    let stderr = String::from_utf8_lossy(stderr);
    let (major, minor, patch) = PYTHON_BOUND
        .captures_iter(&stderr)
        .filter_map(|caps| Some(version_sort_key(caps.get(1)?.as_str())))
        .max()?;

    Some(semver::Version::new(major, minor, patch))
}

/// Whether the hook's original request permits the `version` uv wants to upgrade to. The default
/// and metadata-derived ranges (e.g. pyproject `requires-python`) permit compatible upgrades; an
/// explicit pin or bound that excludes `version` does not, so its error surfaces instead.
fn request_permits(original: &LanguageRequest, version: &semver::Version) -> bool {
    match original {
        LanguageRequest::Any { .. } => true,
        LanguageRequest::Python(request) => request.permits(version),
        // Non-Python requests never reach the Python retry path.
        _ => false,
    }
}

/// The venv request for a retry: `>=bound`, plus any upper bound or pin the original request
/// still imposes (e.g. `>=3.8, <3.12` caps a retry to `>=3.11.2, <3.12`, not an open `>=3.11.2`
/// that could let uv pick a 3.12+ interpreter the hook explicitly excluded).
fn retry_request_for(original: &LanguageRequest, bound: &semver::Version) -> LanguageRequest {
    let mut comparators = vec![format!(">={bound}")];
    match original {
        // `request_permits` only accepts an exact-pin original when `bound` already equals it.
        LanguageRequest::Python(PythonRequest::MajorMinorPatch(..)) => {
            comparators = vec![format!("={bound}")];
        }
        LanguageRequest::Python(PythonRequest::Major(major)) => {
            comparators.push(format!("<{}.0.0", major + 1));
        }
        LanguageRequest::Python(PythonRequest::MajorMinor(major, minor)) => {
            comparators.push(format!("<{major}.{}.0", minor + 1));
        }
        LanguageRequest::Python(PythonRequest::Range(version_req, _)) => {
            comparators.extend(
                version_req
                    .comparators
                    .iter()
                    .filter(|c| matches!(c.op, semver::Op::Less | semver::Op::LessEq))
                    .map(ToString::to_string),
            );
        }
        _ => {}
    }
    let raw = comparators.join(", ");
    let version_req =
        semver::VersionReq::parse(&raw).expect("comparators built from a Version are valid");
    LanguageRequest::Python(PythonRequest::Range(version_req, raw))
}

/// Sort key for a `major.minor[.patch]` version string; unparsable parts sort as 0.
fn version_sort_key(version: &str) -> (u64, u64, u64) {
    let mut parts = version.split('.').map(|p| p.parse::<u64>().unwrap_or(0));
    (
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
        parts.next().unwrap_or(0),
    )
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
            .isolate_from_git_env()
            .check(true);
        cmd
    }

    /// Install the hook's dependencies, retrying once with the Python version inferred from
    /// `uv`'s resolution error (a dependency's `requires-python`). On no inferable or permitted
    /// version, the original error is surfaced.
    async fn install_dependencies(
        uv: &Uv,
        store: &Store,
        info: &InstallInfo,
        hook: &Hook,
        python_request: &LanguageRequest,
    ) -> Result<()> {
        if hook.repo_path().is_none() && hook.additional_dependencies.is_empty() {
            debug!("No dependencies to install");
            return Ok(());
        }

        let build = || {
            let mut cmd = Self::pip_install_command(uv, store, &info.env_path);
            if let Some(repo_path) = hook.repo_path() {
                cmd.arg("--directory").arg(repo_path).arg(".");
            }
            cmd.args(&hook.additional_dependencies);
            cmd
        };

        // Capture the failure instead of bailing, so we can inspect and maybe retry.
        let mut cmd = build();
        let output = cmd.check(false).output().await?;
        if output.status.success() {
            return Ok(());
        }

        // Retry only when downloading is allowed and the hook's original request still permits
        // the inferred interpreter, so a pin or bound the user (or pyproject metadata) set is
        // never overridden; otherwise surface the original resolution error.
        let retry_bound = infer_python_request(&output.stderr)
            .filter(|_| python_request.allows_download())
            .filter(|bound| request_permits(python_request, bound));

        let Some(bound) = retry_bound else {
            // `output.status.success()` is already known false here, so this always errors.
            return cmd.check_output(output).map(|_| ()).map_err(Into::into);
        };
        let retry_request = retry_request_for(python_request, &bound);

        // Preserve the original resolution error if the venv recreate fails.
        let original_error = String::from_utf8_lossy(&output.stderr).into_owned();
        debug!("uv pip install failed to resolve; retrying with the inferred Python version");
        // Recreate the venv from scratch so the retry deterministically uses the new interpreter.
        if info.env_path.exists() {
            fs_err::tokio::remove_dir_all(&info.env_path)
                .await
                .context("Failed to remove venv before retry")?;
        }
        Self::create_venv(uv, store, info, &retry_request)
            .await
            .with_context(|| {
                format!(
                    "Failed to recreate the venv with the inferred Python version.\n\
                     Original dependency resolution error:\n{original_error}"
                )
            })?;
        build().check(true).output().await?;
        Ok(())
    }

    async fn create_venv(
        uv: &Uv,
        store: &Store,
        info: &InstallInfo,
        python_request: &LanguageRequest,
    ) -> Result<()> {
        // Prefer Python installations already managed by prek.
        match Self::create_venv_command(uv, store, info, python_request, VenvAttempt::PrekManaged)
            .check(true)
            .output()
            .await
        {
            Ok(_) => {
                debug!(
                    "Venv created with prek-managed Python: `{}`",
                    info.env_path.display()
                );
                return Ok(());
            }
            Err(process::Error::Status { .. }) => {}
            Err(e) => {
                return Err(e.into());
            }
        }

        // Next, use uv's normal discovery outside prek's managed store.
        match Self::create_venv_command(uv, store, info, python_request, VenvAttempt::External)
            .check(true)
            .output()
            .await
        {
            Ok(_) => {
                debug!(
                    "Venv created with Python discovered outside prek's managed store: `{}`",
                    info.env_path.display()
                );
                Ok(())
            }
            Err(e @ process::Error::Status { .. }) => {
                if Self::can_retry_with_downloads(&e) {
                    if !python_request.allows_download() {
                        anyhow::bail!(
                            "No suitable system Python version found and downloads are disabled"
                        );
                    }

                    debug!(
                        "Downloading Python into prek's managed store: `{}`",
                        info.env_path.display()
                    );
                    Self::create_venv_command(
                        uv,
                        store,
                        info,
                        python_request,
                        VenvAttempt::Download,
                    )
                    .check(true)
                    .output()
                    .await?;
                    return Ok(());
                }
                // If we can't retry, return the original error
                Err(e.into())
            }
            Err(e) => {
                debug!("Failed to create venv `{}`: {e}", info.env_path.display());
                Err(e.into())
            }
        }
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

    #[test]
    fn infer_python_request_picks_highest_bound() {
        use super::infer_python_request;

        // No Python bound in the error -> nothing to refine.
        assert!(infer_python_request(b"error: something unrelated failed").is_none());

        // A single bound.
        let bound = infer_python_request(b"Because foo requires Python >=3.10, ...").unwrap();
        assert_eq!(bound, semver::Version::new(3, 10, 0));

        // Multiple bounds -> the highest wins (and beats lexical: 3.9 < 3.10), keeping the patch.
        let bound =
            infer_python_request(b"requires Python >=3.9 and bar requires Python>=3.11.2 so ...")
                .unwrap();
        assert_eq!(bound, semver::Version::new(3, 11, 2));
    }

    #[test]
    fn request_permits_honors_original_bounds() {
        use super::request_permits;
        use crate::languages::python::PythonRequest;

        let bound = semver::Version::new(3, 11, 2);

        // The default and metadata-derived ranges permit a compatible upgrade.
        assert!(request_permits(
            &LanguageRequest::Any { system_only: false },
            &bound
        ));
        let derived = LanguageRequest::Python(">=3.8".parse::<PythonRequest>().unwrap());
        assert!(request_permits(&derived, &bound));

        // A pin, or a cap the bound violates, does not (the patch matters: `<3.11.2` excludes it).
        let pinned = LanguageRequest::Python(PythonRequest::MajorMinor(3, 9));
        assert!(!request_permits(&pinned, &bound));
        let capped = LanguageRequest::Python(">=3.8, <3.11.2".parse::<PythonRequest>().unwrap());
        assert!(!request_permits(&capped, &bound));
    }

    #[test]
    fn retry_request_preserves_the_patch_bound() {
        use super::{retry_request_for, to_uv_python_request};

        // A patch-bearing bound must survive into the venv request; a `major.minor` pin would
        // let external discovery settle for an older, non-conforming patch (e.g. an installed
        // 3.12.0 when the dependency actually requires >=3.12.5).
        let any = LanguageRequest::Any { system_only: false };
        let request = retry_request_for(&any, &semver::Version::new(3, 12, 5));
        assert_eq!(to_uv_python_request(&request).as_deref(), Some(">=3.12.5"));
    }

    #[test]
    fn retry_request_preserves_the_original_upper_bound() {
        use super::retry_request_for;
        use crate::languages::python::PythonRequest;

        let bound = semver::Version::new(3, 11, 2);

        // An explicit upper-bounded range keeps its cap, so uv can't pick a 3.12+ interpreter
        // the hook excluded.
        let capped = LanguageRequest::Python(">=3.8, <3.12".parse::<PythonRequest>().unwrap());
        let request = retry_request_for(&capped, &bound);
        let LanguageRequest::Python(PythonRequest::Range(req, _)) = &request else {
            panic!("expected a Range request");
        };
        assert!(req.matches(&semver::Version::new(3, 11, 5)));
        assert!(!req.matches(&semver::Version::new(3, 12, 0)));
        assert!(!req.matches(&semver::Version::new(3, 11, 1)));

        // A `major.minor` pin caps the retry to that minor line.
        let pinned = LanguageRequest::Python(PythonRequest::MajorMinor(3, 11));
        let request = retry_request_for(&pinned, &bound);
        let LanguageRequest::Python(PythonRequest::Range(req, _)) = &request else {
            panic!("expected a Range request");
        };
        assert!(req.matches(&semver::Version::new(3, 11, 9)));
        assert!(!req.matches(&semver::Version::new(3, 12, 0)));
    }
}
