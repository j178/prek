use std::collections::BTreeSet;
use std::hash::{Hash, Hasher};
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;
use serde::{Deserialize, Serialize};

use crate::config::{Config, Language, PythonUvLockMode, PythonUvOptions};
use crate::hook::HookSpec;
use crate::languages::version::LanguageRequest;

/// Resolved installer inputs for a hook environment.
///
/// This keeps language-specific installation data, such as `python_uv` project
/// settings, next to the exact identity used for environment reuse. It does not
/// perform installation itself.
#[derive(Debug, Clone)]
pub(crate) enum HookEnvSpec {
    Dependencies(DependencyEnvIdentity),
    PythonUv {
        env: PythonUvEnv,
        identity: PythonUvEnvIdentity,
    },
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
            let (env, identity) = PythonUvEnv::resolve(uv, project_root)?;
            Ok(Self::PythonUv { env, identity })
        } else {
            if uv.is_some() {
                anyhow::bail!(
                    "Hook specified `uv` options but the language `{language}` is not `python_uv`",
                );
            }

            validate_additional_dependencies(language, additional_dependencies)?;

            Ok(Self::Dependencies(DependencyEnvIdentity::new(
                additional_dependencies,
                remote_repo_dependency,
            )))
        }
    }

    pub(crate) fn identity(&self) -> HookEnvIdentityRef<'_> {
        match self {
            Self::Dependencies(identity) => HookEnvIdentityRef::Dependencies(identity),
            Self::PythonUv { identity, .. } => HookEnvIdentityRef::PythonUv(identity),
        }
    }

    pub(crate) fn python_uv(&self) -> Option<&PythonUvEnv> {
        match self {
            Self::Dependencies(_) => None,
            Self::PythonUv { env, .. } => Some(env),
        }
    }
}

/// Exact, persisted identity of an installed hook environment.
///
/// Unlike [`HookEnvRequest`], this is not a selector: two identities are either
/// equal or they describe different environment contents. It intentionally
/// separates install-time dependencies from other language-specific fingerprints
/// such as `python_uv` lockfile state.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", content = "data", rename_all = "snake_case")]
pub(crate) enum HookEnvIdentity {
    Dependencies(DependencyEnvIdentity),
    PythonUv(PythonUvEnvIdentity),
}

impl HookEnvIdentity {
    #[cfg(test)]
    pub(crate) fn empty_dependencies() -> Self {
        Self::Dependencies(DependencyEnvIdentity::new(&FxHashSet::default(), None))
    }
}

/// Borrowed view of an environment identity.
///
/// Use this for matching and grouping. Convert to [`HookEnvIdentity`] only when
/// the identity must outlive the resolved hook, such as when writing install
/// metadata or collecting cache-GC requests.
#[derive(Debug, Clone, Copy, Hash)]
pub(crate) enum HookEnvIdentityRef<'a> {
    Dependencies(&'a DependencyEnvIdentity),
    PythonUv(&'a PythonUvEnvIdentity),
}

impl From<HookEnvIdentityRef<'_>> for HookEnvIdentity {
    fn from(identity: HookEnvIdentityRef<'_>) -> Self {
        match identity {
            HookEnvIdentityRef::Dependencies(identity) => Self::Dependencies(identity.clone()),
            HookEnvIdentityRef::PythonUv(identity) => Self::PythonUv(identity.clone()),
        }
    }
}

impl<'a> From<&'a HookEnvIdentity> for HookEnvIdentityRef<'a> {
    fn from(identity: &'a HookEnvIdentity) -> Self {
        match identity {
            HookEnvIdentity::Dependencies(identity) => Self::Dependencies(identity),
            HookEnvIdentity::PythonUv(identity) => Self::PythonUv(identity),
        }
    }
}

impl Eq for HookEnvIdentityRef<'_> {}

impl<'a, 'b> PartialEq<HookEnvIdentityRef<'b>> for HookEnvIdentityRef<'a> {
    fn eq(&self, other: &HookEnvIdentityRef<'b>) -> bool {
        match (self, other) {
            (Self::Dependencies(left), HookEnvIdentityRef::Dependencies(right)) => left == right,
            (Self::PythonUv(left), HookEnvIdentityRef::PythonUv(right)) => left == right,
            _ => false,
        }
    }
}

/// Identity for languages whose environment is determined by dependency specs.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct DependencyEnvIdentity {
    /// User-provided dependency specs that affect the managed hook environment.
    pub(crate) dependencies: BTreeSet<String>,
    /// Remote hook repository identity, included because the repository package
    /// itself is installed into many language environments.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) remote_repo: Option<String>,
}

impl DependencyEnvIdentity {
    fn new(additional_dependencies: &FxHashSet<String>, remote_repo: Option<&str>) -> Self {
        Self {
            dependencies: additional_dependencies.iter().cloned().collect(),
            remote_repo: remote_repo.map(str::to_string),
        }
    }
}

/// Identity for `language: python_uv` environments.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub(crate) struct PythonUvEnvIdentity {
    /// Versioned digest of uv project inputs that affect `uv sync`.
    pub(crate) fingerprint: String,
}

/// Environment lookup request derived from hook configuration.
///
/// This is not an exact key: `language_request` may be a range or `default`,
/// so an installed environment satisfies the request only after checking its
/// actual language version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HookEnvRequest {
    pub(crate) language: Language,
    pub(crate) identity: HookEnvIdentity,
    pub(crate) language_request: LanguageRequest,
}

impl HookEnvRequest {
    /// Compute the environment request used to find reusable installed envs.
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
        let identity = env_spec.identity().into();

        Ok(Some(Self {
            language,
            identity,
            language_request,
        }))
    }
}

/// Borrowed environment lookup request.
///
/// This avoids cloning exact identity data when a resolved hook is only being
/// compared against installed environments.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HookEnvRequestRef<'a> {
    pub(crate) language: Language,
    pub(crate) identity: HookEnvIdentityRef<'a>,
    pub(crate) language_request: &'a LanguageRequest,
}

impl<'a> From<&'a HookEnvRequest> for HookEnvRequestRef<'a> {
    fn from(request: &'a HookEnvRequest) -> Self {
        Self {
            language: request.language,
            identity: HookEnvIdentityRef::from(&request.identity),
            language_request: &request.language_request,
        }
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
}

impl PythonUvEnv {
    const SCHEMA_VERSION: &'static str = "python_uv_v1";

    pub(crate) fn resolve(
        options: &PythonUvOptions,
        project_root: &Path,
    ) -> Result<(Self, PythonUvEnvIdentity)> {
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

        let identity = python_uv_env_identity(
            &project,
            &lockfile,
            &pyproject,
            &dependency_groups,
            &extras,
            install_project,
            lock_mode,
        )?;

        Ok((
            Self {
                project,
                lockfile,
                dependency_groups,
                extras,
                install_project,
                lock_mode,
            },
            identity,
        ))
    }
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

fn python_uv_env_identity(
    project: &Path,
    lockfile: &Path,
    pyproject: &Path,
    dependency_groups: &[String],
    extras: &[String],
    install_project: bool,
    lock_mode: PythonUvLockMode,
) -> Result<PythonUvEnvIdentity> {
    let key = PythonUvEnvCacheKey {
        schema: PythonUvEnv::SCHEMA_VERSION,
        source: PythonUvEnvSourceCacheKey::UvLock {
            project,
            lockfile,
            lockfile_hash: hash_file_contents(lockfile)?,
            pyproject_hash: hash_file_contents(pyproject)?,
            uv_toml_hash: hash_optional_file(&project.join("uv.toml"))?,
            python_version_file_hash: hash_optional_file(&project.join(".python-version"))?,
        },
        dependency_groups,
        extras,
        install_project,
        lock_mode,
    };

    Ok(PythonUvEnvIdentity {
        fingerprint: format!("{}:{}", PythonUvEnv::SCHEMA_VERSION, hash_digest(&key)),
    })
}

#[derive(Hash)]
struct PythonUvEnvCacheKey<'a> {
    schema: &'static str,
    source: PythonUvEnvSourceCacheKey<'a>,
    dependency_groups: &'a [String],
    extras: &'a [String],
    install_project: bool,
    lock_mode: PythonUvLockMode,
}

#[derive(Hash)]
enum PythonUvEnvSourceCacheKey<'a> {
    UvLock {
        project: &'a Path,
        lockfile: &'a Path,
        lockfile_hash: u64,
        pyproject_hash: u64,
        uv_toml_hash: Option<u64>,
        python_version_file_hash: Option<u64>,
    },
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

fn hash_optional_file(path: &Path) -> Result<Option<u64>> {
    match hash_file_contents(path) {
        Ok(hash) => Ok(Some(hash)),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err.into()),
    }
}

fn hash_file_contents(path: &Path) -> io::Result<u64> {
    let mut file = fs_err::File::open(path)?;
    let mut hasher = seahash::SeaHasher::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(hasher.finish())
}

fn hash_digest<T: Hash + ?Sized>(value: &T) -> String {
    let mut hasher = seahash::SeaHasher::new();
    value.hash(&mut hasher);
    hex::encode(hasher.finish().to_le_bytes())
}
