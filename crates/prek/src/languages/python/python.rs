use std::env::consts::EXE_EXTENSION;
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use anyhow::{Context, Result};
use asyncband::once::OnceMap;
use prek_consts::env_vars::EnvVars;
use prek_consts::prepend_paths;
use regex::Regex;
use rustc_hash::FxBuildHasher;
use tracing::{debug, trace};

use crate::cli::reporter::HookInstallReporter;
use crate::git::GitCommandExt;
use crate::hook::InstalledHook;
use crate::hook::{Hook, InstallInfo};
use crate::languages::python::PythonRequest;
use crate::languages::python::uv::Uv;
use crate::languages::version::{LanguageRequest, ToolchainSource, VersionRequest};
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

impl PythonInfo {
    fn parse(output: &[u8]) -> Result<Self> {
        let output = std::str::from_utf8(output)?;
        // The prefix may contain newlines or trailing whitespace.
        let (version, base_exec_prefix) = output
            .split_once('\n')
            .context("Missing Python version separator")?;
        Ok(Self {
            version: version.parse()?,
            python_exec: python_exec(Path::new(base_exec_prefix)),
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum PythonInfoError {
    #[error("Failed to parse Python info: {0}")]
    Parse(String),
    #[error("Failed to query Python info: {0}")]
    Query(String),
}

// Canonical paths let virtual environments backed by the same interpreter share one query.
static PYTHON_INFO_CACHE: LazyLock<OnceMap<PathBuf, Arc<PythonInfo>, FxBuildHasher>> =
    LazyLock::new(|| OnceMap::with_hasher(FxBuildHasher));

async fn query_python_info(python: &Path) -> Result<PythonInfo, PythonInfoError> {
    static QUERY_PYTHON_INFO: &str = indoc::indoc! {r#"
    import sys
    info = ".".join(map(str, sys.version_info[:3])) + "\n" + sys.base_exec_prefix
    sys.stdout.buffer.write(info.encode("utf-8"))
    "#};

    let stdout = Cmd::new(python)
        .arg("-I")
        .arg("-S")
        .arg("-c")
        .arg(QUERY_PYTHON_INFO)
        .check(true)
        .output()
        .await
        .map_err(|err| PythonInfoError::Query(err.to_string()))?
        .stdout;

    PythonInfo::parse(&stdout).map_err(|err| PythonInfoError::Parse(err.to_string()))
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
        Self::install_dependencies(
            &uv,
            store,
            &info,
            &hook,
            install_cwd,
            &hook.language_request,
        )
        .await?;

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

/// A dependency's `requires-python` bound from a uv resolution failure.
struct PythonBound {
    version: semver::Version,
    /// A strict `>` bound, which a retry has to keep: `>=` can settle on the interpreter that
    /// just failed to resolve.
    exclusive: bool,
    /// The cap uv stated for the same requirement (`Python>=3.12,<3.13`), if it stated one. An
    /// open retry off that lower bound could settle on an interpreter the dependency rejects.
    upper: Option<PythonUpper>,
}

/// The upper half of a `requires-python` bound, e.g. the `<3.13` of `Python>=3.12,<3.13`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PythonUpper {
    version: semver::Version,
    exclusive: bool,
}

impl PythonBound {
    /// Whether `version` is inside the range uv named, which is the range a retry would search.
    fn allows(&self, version: &semver::Version) -> bool {
        let above_lower = if self.exclusive {
            version > &self.version
        } else {
            version >= &self.version
        };
        let below_upper = match &self.upper {
            Some(upper) if upper.exclusive => version < &upper.version,
            Some(upper) => version <= &upper.version,
            None => true,
        };
        above_lower && below_upper
    }

    /// The cap a retry adds to the inferred bound, the next minor line.
    ///
    /// uv refines an inferred request the same way: an open `>=3.12` would otherwise let the
    /// interpreter search take the newest release, which the dependency may have no wheels for,
    /// instead of the oldest line that can satisfy it.
    fn minor_cap(&self) -> semver::Version {
        semver::Version::new(self.version.major, self.version.minor + 1, 0)
    }

    /// The first version inside the range, or just above a strict bound.
    fn first_version(&self) -> semver::Version {
        if self.exclusive {
            semver::Version::new(
                self.version.major,
                self.version.minor,
                self.version.patch + 1,
            )
        } else {
            self.version.clone()
        }
    }
}

/// The highest `requires-python` bound in a uv resolution failure, patch included so the
/// compatibility check against the hook's original request stays sound (e.g. against `<3.11.2`).
fn infer_python_request(stderr: &[u8]) -> Option<PythonBound> {
    /// A `major[.minor[.patch]]` version as uv writes it in a `requires-python` bound.
    const VERSION: &str = r"\d+(?:\.\d+){0,2}";

    static PYTHON_BOUND: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"Python\s*(>=?)\s*({VERSION})(?:\s*,\s*(<=?)\s*({VERSION}))?"
        ))
        .unwrap()
    });

    let stderr = String::from_utf8_lossy(stderr);
    let mut best: Option<((u64, u64, u64, bool), PythonBound)> = None;
    for caps in PYTHON_BOUND.captures_iter(&stderr) {
        let Some((op, version)) = caps.get(1).zip(caps.get(2)) else {
            continue;
        };
        let (major, minor, patch) = version_sort_key(version.as_str());
        let exclusive = op.as_str() == ">";

        // The highest bound wins, and between equal versions the strict one is the stronger
        // constraint.
        let key = (major, minor, patch, exclusive);
        if best.as_ref().is_some_and(|(best_key, _)| *best_key >= key) {
            continue;
        }

        // A cap is read from the requirement the bound came from: another requirement's cap
        // says nothing about this one.
        let upper = caps.get(3).and_then(|op| {
            let (major, minor, patch) = version_sort_key(caps.get(4)?.as_str());
            Some(PythonUpper {
                version: semver::Version::new(major, minor, patch),
                exclusive: op.as_str() == "<",
            })
        });

        best = Some((
            key,
            PythonBound {
                version: semver::Version::new(major, minor, patch),
                exclusive,
                upper,
            },
        ));
    }

    best.map(|(_, bound)| bound)
}

/// Whether the hook's original request permits the interpreter uv wants to move to. The default
/// and metadata-derived ranges (e.g. pyproject `requires-python`) permit compatible upgrades; an
/// explicit pin or bound that excludes it does not, so its error surfaces instead.
fn request_permits(original: &LanguageRequest, bound: &PythonBound) -> bool {
    let request: &PythonRequest = original.version();
    // An exact pin is only retried with the same version, so a strict bound it does not satisfy
    // has no retry to offer.
    if matches!(request, PythonRequest::MajorMinorPatch(..)) {
        return !bound.exclusive && request.permits(&bound.version);
    }

    // The retry keeps the original's bounds and adds the inferred ones, so it is usable while the
    // two ranges overlap. The overlap starts at the stricter of the two lower bounds, and that
    // version has to clear the caps on both sides: testing the inferred bound alone would reject
    // an original `>=3.12.5` retried at `>=3.12, <3.13`, and accepting it blindly would build the
    // empty `>3.11.2, <=3.11.2` when the original caps at the inferred bound.
    let candidate = match lower_bound(request) {
        Some(lower) => lower.max(bound.first_version()),
        None => bound.first_version(),
    };
    request.permits(&candidate) && bound.allows(&candidate) && candidate < bound.minor_cap()
}

/// The lowest version a request accepts, when it states one.
///
/// A strict `>` is reported as the version itself: the retry it feeds is then conservative,
/// offering no candidate the original would refuse.
fn lower_bound(request: &PythonRequest) -> Option<semver::Version> {
    match request {
        PythonRequest::Any => None,
        PythonRequest::Major(major) => Some(semver::Version::new(*major, 0, 0)),
        PythonRequest::MajorMinor(major, minor) => Some(semver::Version::new(*major, *minor, 0)),
        PythonRequest::MajorMinorPatch(major, minor, patch) => {
            Some(semver::Version::new(*major, *minor, *patch))
        }
        PythonRequest::Range(version_req, _) => version_req
            .comparators
            .iter()
            .filter(|comparator| !matches!(comparator.op, semver::Op::Less | semver::Op::LessEq))
            .map(|comparator| {
                semver::Version::new(
                    comparator.major,
                    comparator.minor.unwrap_or(0),
                    comparator.patch.unwrap_or(0),
                )
            })
            .max(),
    }
}

/// The venv request for a retry: the inferred bound, plus every comparator the original request
/// imposed (e.g. `>=3.8, <3.12` caps `>=3.11.2` to `>=3.11.2, <3.12`, a dependency's own
/// `Python>=3.12,<3.13` keeps its `<3.13`, and a lower bound stronger than the inferred one still
/// applies). The original's download policy is kept.
fn retry_request_for(original: &LanguageRequest, bound: &PythonBound) -> LanguageRequest {
    let request: &PythonRequest = original.version();

    let version = if let PythonRequest::MajorMinorPatch(major, minor, patch) = request {
        // An exact pin is retried as itself, which uv spells `3.11.2` rather than `=3.11.2`.
        PythonRequest::MajorMinorPatch(*major, *minor, *patch)
    } else {
        let op = if bound.exclusive { ">" } else { ">=" };
        let mut comparators = vec![format!("{op}{}", bound.version)];
        comparators.push(format!("<{}", bound.minor_cap()));
        if let Some(upper) = &bound.upper {
            let op = if upper.exclusive { "<" } else { "<=" };
            comparators.push(format!("{op}{}", upper.version));
        }
        match request {
            PythonRequest::Major(major) => comparators.push(format!("<{}.0.0", major + 1)),
            PythonRequest::MajorMinor(major, minor) => {
                comparators.push(format!("<{major}.{}.0", minor + 1));
            }
            PythonRequest::Range(version_req, _) => {
                comparators.extend(version_req.comparators.iter().map(ToString::to_string));
            }
            _ => {}
        }
        let raw = comparators.join(", ");
        let version_req =
            semver::VersionReq::parse(&raw).expect("comparators built from a Version are valid");
        PythonRequest::Range(version_req, raw)
    };

    let mut retry = original.clone();
    retry.set_version(VersionRequest::Python(version));
    retry
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
            .sanitize_git_repo_env()
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
        install_cwd: &Path,
        python_request: &LanguageRequest,
    ) -> Result<()> {
        if hook.repo_path().is_none() && hook.additional_dependencies.is_empty() {
            debug!("No dependencies to install");
            return Ok(());
        }

        let build = || {
            let mut cmd = Self::pip_install_command(uv, store, &info.env_path);
            cmd.current_dir(install_cwd);
            if let Some(repo_path) = hook.repo_path() {
                trace!(
                    "Installing dependencies from repo path: {}",
                    repo_path.display()
                );
                cmd.arg("--directory").arg(repo_path).arg(".");
            } else if !hook.additional_dependencies.is_empty() {
                trace!(
                    "Installing additional dependencies: {:?}",
                    hook.additional_dependencies
                );
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

        // Retry when the hook's original request still permits the inferred interpreter, so a
        // pin or bound the user (or pyproject metadata) set is never overridden; otherwise
        // surface the original resolution error. The retry is not gated on the download policy:
        // `create_venv` enforces that itself, and a constrained request can be met by an
        // interpreter that is already installed, which the failed attempt may not have picked.
        let retry_bound = infer_python_request(&output.stderr)
            .filter(|bound| request_permits(python_request, bound));

        let Some(bound) = retry_bound else {
            // `output.status.success()` is already known false here, so this always errors.
            return cmd.check_output(output).map(|_| ()).map_err(Into::into);
        };

        // The failing message may name a bound the environment's own interpreter already meets,
        // in which case the resolution failed for some other reason and a venv built for the same
        // range would fail identically. Replacing an environment that works cannot help, so
        // surface the original error.
        let current = query_python_info(&python_exec(&info.env_path)).await?;
        if bound.allows(&current.version) {
            return cmd.check_output(output).map(|_| ()).map_err(Into::into);
        }
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
    use std::path::{Path, PathBuf};

    use super::{Python, PythonBound, PythonInfo, PythonUpper, VenvAttempt, python_exec};
    use crate::config::Language;
    use crate::hook::InstallInfo;
    use crate::languages::python::uv::Uv;
    use crate::languages::version::LanguageRequest;
    use crate::store::{Store, ToolBucket};
    use prek_consts::env_vars::EnvVars;

    /// A dependency bound as it would be inferred from a uv error: `>=`, unless strict, and
    /// nothing said about the top of the range.
    fn bound(major: u64, minor: u64, patch: u64, exclusive: bool) -> PythonBound {
        PythonBound {
            version: semver::Version::new(major, minor, patch),
            exclusive,
            upper: None,
        }
    }

    /// The same, for a requirement uv spelled out in full, e.g. `>=3.12, <3.13`.
    fn bounded(lower: (u64, u64, u64, bool), upper: (u64, u64, u64, bool)) -> PythonBound {
        PythonBound {
            version: semver::Version::new(lower.0, lower.1, lower.2),
            exclusive: lower.3,
            upper: Some(PythonUpper {
                version: semver::Version::new(upper.0, upper.1, upper.2),
                exclusive: upper.3,
            }),
        }
    }

    #[test]
    fn python_info_preserves_prefix() -> anyhow::Result<()> {
        for prefix in ["/tmp/Python 安装", "/tmp/Python\n ", r"C:\Users\Jo\Python"] {
            let output = format!("3.14.0\n{prefix}");
            let info = PythonInfo::parse(output.as_bytes())?;

            assert_eq!(info.version, semver::Version::new(3, 14, 0));
            assert_eq!(info.python_exec, python_exec(Path::new(prefix)));
        }
        Ok(())
    }

    #[test]
    fn python_info_rejects_malformed_output() {
        for output in [
            b"3.14.0".as_slice(),
            b"invalid\n/tmp/python",
            b"3.14.0\n\xff",
        ] {
            assert!(PythonInfo::parse(output).is_err());
        }
    }

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
        assert_eq!(bound.version, semver::Version::new(3, 10, 0));
        assert!(!bound.exclusive);

        // Multiple bounds -> the highest wins (and beats lexical: 3.9 < 3.10), keeping the patch.
        let bound =
            infer_python_request(b"requires Python >=3.9 and bar requires Python>=3.11.2 so ...")
                .unwrap();
        assert_eq!(bound.version, semver::Version::new(3, 11, 2));

        // A strict bound stays strict, or the retry could settle on the version uv rejected.
        let bound = infer_python_request(b"bar requires Python>3.10.20 so ...").unwrap();
        assert_eq!(bound.version, semver::Version::new(3, 10, 20));
        assert!(bound.exclusive);

        // Between equal versions the strict one is the constraint that binds.
        let bound = infer_python_request(b"Python>=3.12 and Python>3.12 so ...").unwrap();
        assert!(bound.exclusive);

        // A major-only bound is valid in `Requires-Python`, and is padded like any other.
        let bound = infer_python_request(b"Because foo requires Python>=4, ...").unwrap();
        assert_eq!(bound.version, semver::Version::new(4, 0, 0));
        assert!(!bound.exclusive);
        assert_eq!(bound.upper, None);

        // A bound uv spelled out in full keeps its cap, and the cap belongs to the requirement
        // the bound came from rather than to any other one in the message.
        let bound =
            infer_python_request(b"Because foo requires Python>=3.12,<3.13 and Python>=3.9, ...")
                .unwrap();
        assert_eq!(bound.version, semver::Version::new(3, 12, 0));
        assert_eq!(
            bound.upper,
            Some(PythonUpper {
                version: semver::Version::new(3, 13, 0),
                exclusive: true
            })
        );

        // The real wording uv uses, where the same requirement is stated twice.
        let bound = infer_python_request(
            b"cause: Because the current Python version (3.9.6) does not satisfy Python>=3.11 \
              and numpy==2.3.1 depends on Python>=3.11, we can conclude that numpy==2.3.1 cannot be used.",
        )
        .unwrap();
        assert_eq!(bound.version, semver::Version::new(3, 11, 0));
        assert_eq!(bound.upper, None);
    }

    #[test]
    fn request_permits_honors_original_bounds() {
        use super::request_permits;

        // The default and metadata-derived ranges permit a compatible upgrade.
        assert!(request_permits(
            &LanguageRequest::from_config(Language::Python, None).unwrap(),
            &bound(3, 11, 2, false)
        ));
        let derived = LanguageRequest::parse(Language::Python, ">=3.8").unwrap();
        assert!(request_permits(&derived, &bound(3, 11, 2, false)));

        // A pin, or a cap the bound violates, does not (the patch matters: `<3.11.2` excludes it).
        let pinned = LanguageRequest::parse(Language::Python, "3.9").unwrap();
        assert!(!request_permits(&pinned, &bound(3, 11, 2, false)));
        let capped = LanguageRequest::parse(Language::Python, ">=3.8, <3.11.2").unwrap();
        assert!(!request_permits(&capped, &bound(3, 11, 2, false)));

        // An exact pin can be retried as itself, but only a non-strict bound leaves that valid.
        let exact = LanguageRequest::parse(Language::Python, "3.11.2").unwrap();
        assert!(request_permits(&exact, &bound(3, 11, 2, false)));
        assert!(!request_permits(&exact, &bound(3, 11, 2, true)));
    }

    #[test]
    fn bound_allows_only_versions_inside_the_range_uv_named() {
        // A retry is skipped when the interpreter the install used is already inside the range uv
        // asked for, so `allows` has to respect both ends.
        let lower = bound(3, 11, 2, false);
        assert!(lower.allows(&semver::Version::new(3, 11, 2)));
        assert!(lower.allows(&semver::Version::new(3, 12, 0)));
        assert!(!lower.allows(&semver::Version::new(3, 11, 1)));

        let strict = bound(3, 11, 2, true);
        assert!(!strict.allows(&semver::Version::new(3, 11, 2)));
        assert!(strict.allows(&semver::Version::new(3, 11, 3)));

        let capped = bounded((3, 12, 0, false), (3, 13, 0, true));
        assert!(capped.allows(&semver::Version::new(3, 12, 9)));
        assert!(!capped.allows(&semver::Version::new(3, 13, 0)));

        let inclusive = bounded((3, 12, 0, false), (3, 13, 0, false));
        assert!(inclusive.allows(&semver::Version::new(3, 13, 0)));
        assert!(!inclusive.allows(&semver::Version::new(3, 13, 1)));
    }

    #[test]
    fn request_permits_rejects_a_retry_with_an_empty_range() {
        use super::request_permits;

        // A strict bound retries strictly above itself, so an original cap at that same version
        // leaves nothing to install; the bound is still inside the original request, which is
        // why testing the bound alone is not enough.
        let capped = LanguageRequest::parse(Language::Python, "<=3.11.2").unwrap();
        assert!(!request_permits(&capped, &bound(3, 11, 2, true)));

        let capped = LanguageRequest::parse(Language::Python, "<3.11.2").unwrap();
        assert!(!request_permits(&capped, &bound(3, 11, 1, true)));

        // One patch of headroom, and the retry has a candidate again.
        assert!(request_permits(&capped, &bound(3, 11, 0, true)));

        // A non-strict bound is retried at the bound itself, so a cap that keeps it stays valid.
        let capped = LanguageRequest::parse(Language::Python, "<=3.11.2").unwrap();
        assert!(request_permits(&capped, &bound(3, 11, 2, false)));

        // A lower bound stronger than the inferred one is not a conflict: the retry starts at
        // whichever bound is higher, which is what the overlap test has to allow.
        let pinned = LanguageRequest::parse(Language::Python, ">=3.12.5").unwrap();
        assert!(request_permits(
            &pinned,
            &bounded((3, 12, 0, false), (3, 13, 0, true))
        ));

        // A cap exactly at the inferred bound still leaves that one version to install.
        let capped = LanguageRequest::parse(Language::Python, ">=3.8, <=3.11.2").unwrap();
        assert!(request_permits(&capped, &bound(3, 11, 2, false)));
        // A strict inferred bound is above it, so there the cap does leave nothing.
        assert!(!request_permits(&capped, &bound(3, 11, 2, true)));
    }

    #[test]
    fn retry_request_preserves_the_patch_bound() {
        use super::{retry_request_for, to_uv_python_request};

        // A patch-bearing bound must survive into the venv request; a `major.minor` pin would
        // let external discovery settle for an older, non-conforming patch (e.g. an installed
        // 3.12.0 when the dependency actually requires >=3.12.5).
        //
        // The request stops at the inferred minor line, like uv's own refinement, so an open
        // bound cannot be filled by the newest release, which the dependency may have no wheels
        // for.
        let any = LanguageRequest::from_config(Language::Python, None).unwrap();
        let request = retry_request_for(&any, &bound(3, 12, 5, false));
        assert_eq!(
            to_uv_python_request(&request).as_deref(),
            Some(">=3.12.5, <3.13.0")
        );

        // A strict bound keeps its operator, so uv cannot settle for the version that failed.
        let request = retry_request_for(&any, &bound(3, 10, 20, true));
        assert_eq!(
            to_uv_python_request(&request).as_deref(),
            Some(">3.10.20, <3.11.0")
        );

        // The inferred bound alone is capped too, so a retry cannot open beyond its minor line.
        let request = retry_request_for(&any, &bound(3, 12, 0, false));
        assert_eq!(
            to_uv_python_request(&request).as_deref(),
            Some(">=3.12.0, <3.13.0")
        );

        // An exact pin is retried as that version, spelled the way uv accepts it.
        let exact = LanguageRequest::parse(Language::Python, "3.12.5").unwrap();
        let request = retry_request_for(&exact, &bound(3, 12, 5, false));
        assert_eq!(to_uv_python_request(&request).as_deref(), Some("3.12.5"));
    }

    #[test]
    fn retry_request_preserves_the_original_upper_bound() {
        use super::retry_request_for;
        use crate::languages::python::PythonRequest;

        // An explicit upper-bounded range keeps its cap, so uv can't pick a 3.12+ interpreter
        // the hook excluded.
        let capped = LanguageRequest::parse(Language::Python, ">=3.8, <3.12").unwrap();
        let request = retry_request_for(&capped, &bound(3, 11, 2, false));
        let version: &PythonRequest = request.version();
        let PythonRequest::Range(req, _) = version else {
            panic!("expected a Range request");
        };
        assert!(req.matches(&semver::Version::new(3, 11, 5)));
        assert!(!req.matches(&semver::Version::new(3, 12, 0)));
        assert!(!req.matches(&semver::Version::new(3, 11, 1)));

        // A strict bound stays strict inside the range.
        let request = retry_request_for(&capped, &bound(3, 11, 2, true));
        let version: &PythonRequest = request.version();
        let PythonRequest::Range(req, _) = version else {
            panic!("expected a Range request");
        };
        assert!(!req.matches(&semver::Version::new(3, 11, 2)));
        assert!(req.matches(&semver::Version::new(3, 11, 3)));

        // A `major.minor` pin caps the retry to that minor line.
        let pinned = LanguageRequest::parse(Language::Python, "3.11").unwrap();
        let request = retry_request_for(&pinned, &bound(3, 11, 2, false));
        let version: &PythonRequest = request.version();
        let PythonRequest::Range(req, _) = version else {
            panic!("expected a Range request");
        };
        assert!(req.matches(&semver::Version::new(3, 11, 9)));
        assert!(!req.matches(&semver::Version::new(3, 12, 0)));

        // A lower bound the original stated is kept, so the retry cannot settle below it.
        let pinned = LanguageRequest::parse(Language::Python, ">=3.12.5").unwrap();
        let request = retry_request_for(&pinned, &bounded((3, 12, 0, false), (3, 13, 0, true)));
        let version: &PythonRequest = request.version();
        let PythonRequest::Range(req, _) = version else {
            panic!("expected a Range request");
        };
        assert!(!req.matches(&semver::Version::new(3, 12, 4)));
        assert!(req.matches(&semver::Version::new(3, 12, 5)));
        assert!(!req.matches(&semver::Version::new(3, 13, 0)));

        // A cap uv stated is kept too, so an open `>=3.12` cannot be filled by the 3.13 the
        // dependency rejects. The original's tighter cap still wins over a looser one.
        let any = LanguageRequest::from_config(Language::Python, None).unwrap();
        let request = retry_request_for(&any, &bounded((3, 12, 0, false), (3, 13, 0, true)));
        let version: &PythonRequest = request.version();
        let PythonRequest::Range(req, _) = version else {
            panic!("expected a Range request");
        };
        assert!(req.matches(&semver::Version::new(3, 12, 9)));
        assert!(!req.matches(&semver::Version::new(3, 13, 0)));

        let request = retry_request_for(&capped, &bounded((3, 11, 2, false), (3, 12, 0, true)));
        let version: &PythonRequest = request.version();
        let PythonRequest::Range(req, _) = version else {
            panic!("expected a Range request");
        };
        assert!(req.matches(&semver::Version::new(3, 11, 5)));
        assert!(!req.matches(&semver::Version::new(3, 12, 0)));
    }
}
