use std::ffi::OsStr;
use std::fmt::{self, Display};
use std::path::{Path, PathBuf};
use std::str::FromStr;

use crate::config::{Language, LanguageVersion, ToolchainPreference};
use crate::hook::InstallInfo;
use crate::languages::bun::BunRequest;
use crate::languages::deno::DenoRequest;
use crate::languages::dotnet::DotnetRequest;
use crate::languages::golang::GoRequest;
use crate::languages::node::NodeRequest;
use crate::languages::python::PythonRequest;
use crate::languages::ruby::RubyRequest;
use crate::languages::rust::RustRequest;

#[derive(thiserror::Error, Debug)]
pub(crate) enum Error {
    #[error("Invalid `language_version` value: `{0}`")]
    InvalidVersion(String),
}

/// A version constraint together with the policy for acquiring a matching toolchain.
#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct LanguageRequest {
    version: VersionRequest,
    preference: ToolchainPreference,
    allows_download: bool,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) enum ToolchainSource {
    Managed,
    System,
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(crate) struct ToolchainPolicy {
    preference: ToolchainPreference,
    allows_download: bool,
}

impl ToolchainPolicy {
    pub(crate) fn search_order(self) -> &'static [ToolchainSource] {
        match self.preference {
            ToolchainPreference::OnlyManaged => &[ToolchainSource::Managed],
            ToolchainPreference::Managed => &[ToolchainSource::Managed, ToolchainSource::System],
            ToolchainPreference::System => &[ToolchainSource::System, ToolchainSource::Managed],
            ToolchainPreference::OnlySystem => &[ToolchainSource::System],
        }
    }

    pub(crate) fn allows_download(self) -> bool {
        self.allows_download
    }
}

pub(crate) fn find_system_executables(
    binary_name: impl AsRef<OsStr>,
    managed_root: &Path,
) -> which::Result<Vec<PathBuf>> {
    let managed_root =
        dunce::canonicalize(managed_root).unwrap_or_else(|_| managed_root.to_path_buf());
    Ok(which::which_all(binary_name)?
        .filter(|path| {
            let path = dunce::canonicalize(path).unwrap_or_else(|_| path.clone());
            !path.starts_with(&managed_root)
        })
        .collect())
}

impl Display for ToolchainPolicy {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let downloads = if self.allows_download {
            "enabled"
        } else {
            "disabled"
        };
        write!(f, "{} (downloads {downloads})", self.preference)
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum VersionRequest {
    Bun(BunRequest),
    Dotnet(DotnetRequest),
    Deno(DenoRequest),
    Golang(GoRequest),
    Ruby(RubyRequest),
    Node(NodeRequest),
    Python(PythonRequest),
    Rust(RustRequest),
    // TODO: all other languages default to semver for now.
    Semver(SemverRequest),
}

pub(crate) trait LanguageVersionRequest {
    fn from_version_request(request: &VersionRequest) -> &Self;
}

macro_rules! impl_language_version_request {
    ($request:ty, $variant:ident) => {
        impl LanguageVersionRequest for $request {
            fn from_version_request(request: &VersionRequest) -> &Self {
                match request {
                    VersionRequest::$variant(request) => request,
                    _ => unreachable!("language-specific version request mismatch"),
                }
            }
        }
    };
}

impl_language_version_request!(BunRequest, Bun);
impl_language_version_request!(DotnetRequest, Dotnet);
impl_language_version_request!(DenoRequest, Deno);
impl_language_version_request!(GoRequest, Golang);
impl_language_version_request!(RubyRequest, Ruby);
impl_language_version_request!(NodeRequest, Node);
impl_language_version_request!(PythonRequest, Python);
impl_language_version_request!(RustRequest, Rust);
impl_language_version_request!(SemverRequest, Semver);

impl LanguageRequest {
    pub(crate) fn is_any(&self) -> bool {
        self.version.is_any()
    }

    pub(crate) fn toolchain_policy(&self) -> ToolchainPolicy {
        ToolchainPolicy {
            preference: self.preference,
            allows_download: self.allows_download,
        }
    }

    pub(crate) fn version_request(&self) -> &VersionRequest {
        &self.version
    }

    pub(crate) fn version<T: LanguageVersionRequest>(&self) -> &T {
        T::from_version_request(&self.version)
    }

    /// Replace only the version constraint, preserving download policy.
    pub(crate) fn set_version(&mut self, version: VersionRequest) {
        self.version = version;
    }

    pub(crate) fn parse(lang: Language, request: &str) -> Result<Self, Error> {
        let language_version = LanguageVersion::from(request);
        Self::from_config(lang, Some(&language_version))
    }

    pub(crate) fn from_config(
        lang: Language,
        language_version: Option<&LanguageVersion>,
    ) -> Result<Self, Error> {
        let (request, preference, allows_download) = match language_version {
            Some(language_version) => (
                language_version.request().unwrap_or_default(),
                language_version.preference(),
                language_version.allows_download(),
            ),
            None => ("", ToolchainPreference::default(), true),
        };

        Ok(Self {
            version: VersionRequest::parse(lang, request)?,
            preference,
            allows_download,
        })
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        self.version.satisfied_by(install_info)
    }
}

impl VersionRequest {
    pub(crate) fn parse(lang: Language, request: &str) -> Result<Self, Error> {
        Ok(match lang {
            Language::Bun => Self::Bun(request.parse()?),
            Language::Dotnet => Self::Dotnet(request.parse()?),
            Language::Deno => Self::Deno(request.parse()?),
            Language::Golang => Self::Golang(request.parse()?),
            Language::Node => Self::Node(request.parse()?),
            Language::Python => Self::Python(request.parse()?),
            Language::Ruby => Self::Ruby(request.parse()?),
            Language::Rust => Self::Rust(request.parse()?),
            Language::Conda
            | Language::Coursier
            | Language::Dart
            | Language::Docker
            | Language::DockerImage
            | Language::Fail
            | Language::Haskell
            | Language::Julia
            | Language::Lua
            | Language::Mise
            | Language::Perl
            | Language::Php
            | Language::Pygrep
            | Language::R
            | Language::Script
            | Language::Swift
            | Language::System => Self::Semver(request.parse()?),
        })
    }

    fn is_any(&self) -> bool {
        match self {
            Self::Bun(req) => req.is_any(),
            Self::Dotnet(req) => req.is_any(),
            Self::Deno(req) => req.is_any(),
            Self::Golang(req) => req.is_any(),
            Self::Node(req) => req.is_any(),
            Self::Python(req) => req.is_any(),
            Self::Ruby(req) => req.is_any(),
            Self::Rust(req) => req.is_any(),
            Self::Semver(req) => req.is_any(),
        }
    }

    fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            Self::Bun(req) => req.satisfied_by(install_info),
            Self::Dotnet(req) => req.satisfied_by(install_info),
            Self::Deno(req) => req.satisfied_by(install_info),
            Self::Golang(req) => req.satisfied_by(install_info),
            Self::Node(req) => req.satisfied_by(install_info),
            Self::Python(req) => req.satisfied_by(install_info),
            Self::Ruby(req) => req.satisfied_by(install_info),
            Self::Rust(req) => req.satisfied_by(install_info),
            Self::Semver(req) => req.satisfied_by(install_info),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum SemverRequest {
    Any,
    Range(semver::VersionReq),
}

impl FromStr for SemverRequest {
    type Err = Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        if request.is_empty() {
            return Ok(Self::Any);
        }

        semver::VersionReq::parse(request)
            .map(Self::Range)
            .map_err(|_| Error::InvalidVersion(request.to_string()))
    }
}

impl SemverRequest {
    fn is_any(&self) -> bool {
        matches!(self, Self::Any)
    }

    fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        self.matches(&install_info.language_version)
    }

    pub(crate) fn matches(&self, version: &semver::Version) -> bool {
        match self {
            Self::Any => true,
            Self::Range(request) => request.matches(version),
        }
    }
}

pub(crate) fn try_into_u64_slice(version: &str) -> Result<Vec<u64>, std::num::ParseIntError> {
    version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
}

#[cfg(test)]
mod tests {
    use super::{
        LanguageRequest, SemverRequest, ToolchainSource, VersionRequest, find_system_executables,
    };
    use crate::config::{Language, LanguageVersion};
    use crate::fs::make_executable;
    use crate::languages::python::PythonRequest;

    #[test]
    fn default_request_preserves_language() {
        let request = LanguageRequest::parse(Language::Python, "default").unwrap();

        assert_eq!(
            request.version_request(),
            &VersionRequest::Python(PythonRequest::Any)
        );
    }

    #[test]
    fn fallback_default_uses_semver_any() {
        let request = LanguageRequest::parse(Language::Conda, "default").unwrap();

        assert_eq!(
            request.version_request(),
            &VersionRequest::Semver(SemverRequest::Any)
        );
    }

    #[test]
    fn semver_exact_versions_require_equals() {
        let exact: SemverRequest = "=2026.7.18".parse().unwrap();
        let compatible: SemverRequest = "2026.7.18".parse().unwrap();
        let newer = "2026.8.2".parse().unwrap();

        assert!(!exact.matches(&newer));
        assert!(compatible.matches(&newer));
    }

    #[test]
    fn structured_preferences_produce_expected_policies() {
        let cases = [
            ("only-managed", true, &[ToolchainSource::Managed][..]),
            (
                "managed",
                true,
                &[ToolchainSource::Managed, ToolchainSource::System][..],
            ),
            (
                "system",
                true,
                &[ToolchainSource::System, ToolchainSource::Managed][..],
            ),
            ("only-system", false, &[ToolchainSource::System][..]),
        ];

        for (preference, download, search_order) in cases {
            let language_version: LanguageVersion =
                serde_saphyr::from_str(&format!("request: '>=3.12'\npreference: {preference}\n"))
                    .unwrap();
            let request =
                LanguageRequest::from_config(Language::Python, Some(&language_version)).unwrap();
            let policy = request.toolchain_policy();

            assert_eq!(policy.allows_download(), download, "{preference}");
            assert_eq!(policy.search_order(), search_order, "{preference}");
        }
    }

    #[test]
    fn legacy_system_request_keeps_managed_first_fallback_without_downloads() {
        let request = LanguageRequest::parse(Language::Python, "system").unwrap();
        let policy = request.toolchain_policy();

        assert_eq!(
            policy.search_order(),
            &[ToolchainSource::Managed, ToolchainSource::System]
        );
        assert!(!policy.allows_download());
    }

    #[test]
    fn system_executables_exclude_managed_binaries() {
        let temp = tempfile::tempdir().unwrap();
        let managed_root = temp.path().join("managed");
        let binary = managed_root
            .join("bin")
            .join("tool")
            .with_extension(std::env::consts::EXE_EXTENSION);
        fs_err::create_dir_all(binary.parent().unwrap()).unwrap();
        fs_err::write(&binary, "").unwrap();
        make_executable(&binary).unwrap();

        let executables = find_system_executables(&binary, &managed_root).unwrap();

        assert!(executables.is_empty());
    }

    #[test]
    fn system_executables_include_binaries_outside_managed_root() {
        let temp = tempfile::tempdir().unwrap();
        let managed_root = temp.path().join("managed");
        fs_err::create_dir_all(&managed_root).unwrap();
        let binary = temp
            .path()
            .join("system")
            .join("tool")
            .with_extension(std::env::consts::EXE_EXTENSION);
        fs_err::create_dir_all(binary.parent().unwrap()).unwrap();
        fs_err::write(&binary, "").unwrap();
        make_executable(&binary).unwrap();

        let executables = find_system_executables(&binary, &managed_root).unwrap();

        assert_eq!(executables, vec![binary]);
    }

    #[cfg(unix)]
    #[test]
    fn system_executables_exclude_symlinks_to_managed_binaries() {
        let temp = tempfile::tempdir().unwrap();
        let managed_root = temp.path().join("managed");
        let binary = managed_root.join("bin/tool");
        fs_err::create_dir_all(binary.parent().unwrap()).unwrap();
        fs_err::write(&binary, "").unwrap();
        make_executable(&binary).unwrap();
        let link = temp.path().join("tool");
        std::os::unix::fs::symlink(binary, &link).unwrap();

        let executables = find_system_executables(link, &managed_root).unwrap();

        assert!(executables.is_empty());
    }
}
