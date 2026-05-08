use std::hash::Hasher;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustc_hash::{FxBuildHasher, FxHashSet};

use crate::config::{Config, Language, PythonUvLockMode, PythonUvOptions};
use crate::hook::HookSpec;
use crate::languages::version::LanguageRequest;

/// Resolved description of the environment a hook installs into and reuses.
///
/// A hook environment is identified by the language runtime request plus this
/// language-specific dependency set. The spec is the single source of truth for
/// environment reuse, cache GC, and installer inputs; it does not perform
/// installation itself.
#[derive(Debug, Clone)]
pub(crate) enum HookEnvSpec {
    Dependencies(FxHashSet<String>),
    PythonUv(PythonUvEnv),
}

impl HookEnvSpec {
    pub(crate) fn resolve(
        language: Language,
        additional_dependencies: &FxHashSet<String>,
        uv: Option<&PythonUvOptions>,
        project_root: &Path,
        remote_repo_dependency: Option<&str>,
    ) -> Result<Self> {
        if language == Language::PythonUv {
            if !additional_dependencies.is_empty() {
                anyhow::bail!(
                    "`language: python_uv` does not install `additional_dependencies`; add Python packages to a uv dependency group and update `uv.lock` instead",
                );
            }

            let default_uv = PythonUvOptions::default();
            let uv = uv.unwrap_or(&default_uv);
            Ok(Self::PythonUv(PythonUvEnv::resolve(uv, project_root)?))
        } else {
            if uv.is_some() {
                anyhow::bail!(
                    "Hook specified `uv` options but the language `{language}` is not `python_uv`",
                );
            }

            validate_additional_dependencies(language, additional_dependencies)?;

            Ok(Self::Dependencies(env_key_dependencies(
                additional_dependencies,
                remote_repo_dependency,
            )))
        }
    }

    pub(crate) fn dependencies(&self) -> &FxHashSet<String> {
        match self {
            Self::Dependencies(dependencies) => dependencies,
            Self::PythonUv(env) => env.dependencies(),
        }
    }

    pub(crate) fn python_uv(&self) -> Option<&PythonUvEnv> {
        match self {
            Self::Dependencies(_) => None,
            Self::PythonUv(env) => Some(env),
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct HookEnvKey {
    pub(crate) language: Language,
    pub(crate) dependencies: FxHashSet<String>,
    pub(crate) language_request: LanguageRequest,
}

impl HookEnvKey {
    /// Compute the key used to match an installed hook environment.
    ///
    /// Returns `Ok(None)` if this hook does not install an environment.
    pub(crate) fn from_hook_spec(
        config: &Config,
        mut hook_spec: HookSpec,
        remote_repo_dependency: Option<&str>,
        project_root: &Path,
    ) -> Result<Option<Self>> {
        let language = hook_spec.language;
        if !language.supports_install_env() {
            return Ok(None);
        }

        hook_spec.apply_project_defaults(config);
        hook_spec.options.language_version.get_or_insert_default();
        let additional_dependencies = hook_spec
            .options
            .additional_dependencies
            .get_or_insert_default()
            .iter()
            .cloned()
            .collect::<FxHashSet<_>>();

        let request = hook_spec.options.language_version.as_deref().unwrap_or("");
        let language_request = LanguageRequest::parse(language, request).with_context(|| {
            format!(
                "Invalid language_version `{request}` for hook `{}`",
                hook_spec.id
            )
        })?;

        let env_spec = HookEnvSpec::resolve(
            language,
            &additional_dependencies,
            hook_spec.options.uv.as_ref(),
            project_root,
            remote_repo_dependency,
        )?;
        let dependencies = env_spec.dependencies().clone();

        Ok(Some(Self {
            language,
            dependencies,
            language_request,
        }))
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PythonUvEnv {
    pub(crate) project: PathBuf,
    pub(crate) lockfile: PathBuf,
    pub(crate) dependency_groups: Vec<String>,
    pub(crate) extras: Vec<String>,
    pub(crate) install_project: bool,
    pub(crate) lock_mode: PythonUvLockMode,
    dependencies: FxHashSet<String>,
}

impl PythonUvEnv {
    const SCHEMA_VERSION: &'static str = "python_uv_v1";

    pub(crate) fn resolve(options: &PythonUvOptions, project_root: &Path) -> Result<Self> {
        let project = options.project.as_deref().map_or_else(
            || project_root.to_path_buf(),
            |path| resolve_path(project_root, path),
        );
        let project = fs_err::canonicalize(&project).with_context(|| {
            format!(
                "Failed to resolve `uv.project` path `{}`",
                project.display()
            )
        })?;

        let pyproject = project.join("pyproject.toml");
        if !pyproject.is_file() {
            anyhow::bail!(
                "`language: python_uv` requires a uv project at `{}` with a `pyproject.toml`",
                project.display()
            );
        }

        let default_lockfile = project.join("uv.lock");
        let lockfile = options.lockfile.as_deref().map_or_else(
            || default_lockfile.clone(),
            |path| resolve_path(&project, path),
        );
        let lockfile = fs_err::canonicalize(&lockfile).with_context(|| {
            format!(
                "Failed to resolve `uv.lockfile` path `{}`",
                lockfile.display()
            )
        })?;

        let default_lockfile = fs_err::canonicalize(&default_lockfile).with_context(|| {
            format!(
                "`language: python_uv` requires a lockfile at `{}`",
                default_lockfile.display()
            )
        })?;
        if lockfile != default_lockfile {
            anyhow::bail!(
                "`language: python_uv` only supports the project's default uv lockfile (`{}`) for now",
                default_lockfile.display()
            );
        }

        let mut dependency_groups = options.dependency_groups.clone().unwrap_or_default();
        canonicalize_string_list(&mut dependency_groups);

        let mut extras = options.extras.clone().unwrap_or_default();
        canonicalize_string_list(&mut extras);

        let install_project = options.install_project.unwrap_or(true);
        let lock_mode = options.lock_mode.unwrap_or_default();

        let dependencies = python_uv_env_key_dependencies(
            &project,
            &lockfile,
            &pyproject,
            &dependency_groups,
            &extras,
            install_project,
            lock_mode,
        )?;

        Ok(Self {
            project,
            lockfile,
            dependency_groups,
            extras,
            install_project,
            lock_mode,
            dependencies,
        })
    }

    fn dependencies(&self) -> &FxHashSet<String> {
        &self.dependencies
    }
}

/// Builds the dependency set used to identify a hook environment.
///
/// For remote hooks, `remote_repo_dependency` is included so environments from different
/// repositories are not reused accidentally.
fn env_key_dependencies(
    additional_dependencies: &FxHashSet<String>,
    remote_repo_dependency: Option<&str>,
) -> FxHashSet<String> {
    let mut deps = FxHashSet::with_capacity_and_hasher(
        additional_dependencies.len() + usize::from(remote_repo_dependency.is_some()),
        FxBuildHasher,
    );
    deps.extend(additional_dependencies.iter().cloned());
    if let Some(dep) = remote_repo_dependency {
        deps.insert(dep.to_string());
    }
    deps
}

fn validate_additional_dependencies(
    language: Language,
    additional_dependencies: &FxHashSet<String>,
) -> Result<()> {
    if additional_dependencies.is_empty() {
        return Ok(());
    }

    if !language.supports_install_env() {
        anyhow::bail!(
            "Hook specified `additional_dependencies: {}` but the language `{}` does not install an environment",
            format_dependencies(additional_dependencies),
            language,
        );
    }

    if !language.supports_dependency() {
        anyhow::bail!(
            "Hook specified `additional_dependencies: {}` but the language `{}` does not support installing dependencies for now",
            format_dependencies(additional_dependencies),
            language,
        );
    }

    Ok(())
}

fn python_uv_env_key_dependencies(
    project: &Path,
    lockfile: &Path,
    pyproject: &Path,
    dependency_groups: &[String],
    extras: &[String],
    install_project: bool,
    lock_mode: PythonUvLockMode,
) -> Result<FxHashSet<String>> {
    let mut dependencies = FxHashSet::default();
    dependencies.insert(format!("schema={}", PythonUvEnv::SCHEMA_VERSION));
    dependencies.insert(format!("project={}", project.display()));
    dependencies.insert(format!("lockfile={}", lockfile.display()));
    dependencies.insert(format!("lockfile_hash={}", hash_file(lockfile)?));
    dependencies.insert(format!("pyproject_hash={}", hash_file(pyproject)?));
    dependencies.insert(format!("dependency_groups={}", dependency_groups.join(",")));
    dependencies.insert(format!("extras={}", extras.join(",")));
    dependencies.insert(format!("install_project={install_project}"));
    dependencies.insert(format!("lock_mode={lock_mode:?}"));

    push_optional_file_hash(&mut dependencies, "uv_toml_hash", &project.join("uv.toml"))?;
    push_optional_file_hash(
        &mut dependencies,
        "python_version_file_hash",
        &project.join(".python-version"),
    )?;

    Ok(dependencies)
}

fn format_dependencies(dependencies: &FxHashSet<String>) -> String {
    let mut dependencies = dependencies.iter().map(String::as_str).collect::<Vec<_>>();
    dependencies.sort_unstable();
    dependencies.join(", ")
}

fn resolve_path(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

fn canonicalize_string_list(values: &mut Vec<String>) {
    values.sort();
    values.dedup();
}

fn push_optional_file_hash(
    dependencies: &mut FxHashSet<String>,
    key: &str,
    path: &Path,
) -> Result<()> {
    match hash_file(path) {
        Ok(hash) => {
            dependencies.insert(format!("{key}={hash}"));
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => return Err(err.into()),
    }

    Ok(())
}

fn hash_file(path: &Path) -> io::Result<String> {
    let mut file = fs_err::File::open(path)?;
    let mut hasher = seahash::SeaHasher::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:016x}", hasher.finish()))
}
