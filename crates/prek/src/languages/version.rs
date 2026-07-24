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

    /// Whether the request targets the toolchain already installed on the system rather than one
    /// prek manages. Legacy `system` is spelled as a default request with downloads turned off.
    pub(crate) fn prefers_system(self) -> bool {
        !self.allows_download
            || matches!(
                self.preference,
                ToolchainPreference::System | ToolchainPreference::OnlySystem
            )
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

/// Marker an install writes into its install info when an unqualified request selected the
/// interpreter, so that same request can reuse a prerelease it had no alternative to. See
/// [`LanguageRequest::satisfied_by`].
pub(crate) const UNQUALIFIED_REQUEST_KEY: &str = "unqualified_request";

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
        // An unqualified request for Python or Go means a stable interpreter, so it must not
        // silently reuse a prerelease env installed for another hook's explicit request. Asking for
        // the system toolchain is exempt: it takes whatever is on PATH, prerelease or not, and
        // stays reusable once installed. Other languages (Rust, where a nightly or beta toolchain
        // can legitimately be the default) match as before.
        if self.version.is_any()
            && matches!(install_info.language, Language::Python | Language::Golang)
        {
            if install_info.language_version.pre.is_empty() {
                return true;
            }
            // An env this request built itself is the exception: on a machine whose only
            // interpreter is a prerelease, uv has nothing else to offer, and refusing the env it
            // just installed would rebuild it on every run.
            return self.toolchain_policy().prefers_system()
                || install_info.get_extra(UNQUALIFIED_REQUEST_KEY).is_some();
        }
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
/// Parse a compact prerelease version (Go's `1.24rc1`, PEP 440's `3.13.0rc1`) into semver: pad to
/// `major.minor.patch` and map `rc1` -> `rc.1` so `rc.9` < `rc.10`.
pub(crate) fn parse_prerelease_version(s: &str) -> Option<semver::Version> {
    let split = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let (numeric, pre) = s.split_at(split);

    let mut parts = try_into_u64_slice(numeric).ok()?;
    if parts.is_empty() || parts.len() > 3 {
        return None;
    }
    while parts.len() < 3 {
        parts.push(0);
    }

    let pre = if pre.is_empty() {
        semver::Prerelease::EMPTY
    } else {
        // Split the letters from the trailing number: `rc1` -> `rc` + `1`.
        let digit_at = pre.find(|c: char| c.is_ascii_digit()).unwrap_or(pre.len());
        let (label, number) = pre.split_at(digit_at);
        // Real prerelease labels only, so `t` (free-threaded), `-64` (arch), etc. aren't misread.
        const PRERELEASE_LABELS: &[&str] =
            &["a", "b", "c", "rc", "alpha", "beta", "pre", "preview"];
        // A numeric serial is required: `rc1` is valid, but bare `rc` or junk like `rc1foo` is not.
        if !PRERELEASE_LABELS.contains(&label)
            || number.is_empty()
            || !number.bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        semver::Prerelease::new(&format!("{label}.{number}")).ok()?
    };

    Some(semver::Version {
        major: parts[0],
        minor: parts[1],
        patch: parts[2],
        pre,
        build: semver::BuildMetadata::EMPTY,
    })
}

#[cfg(test)]
mod tests {
    use super::{
        LanguageRequest, SemverRequest, ToolchainSource, UNQUALIFIED_REQUEST_KEY, VersionRequest,
        find_system_executables, parse_prerelease_version,
    };
    use crate::config::{Language, LanguageVersion};
    use crate::fs::make_executable;
    use crate::hook::InstallInfo;
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

        assert_eq!(executables, Vec::<std::path::PathBuf>::new());
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

        assert_eq!(executables, Vec::<std::path::PathBuf>::new());
    }

    #[test]
    fn parses_go_and_python_prereleases() {
        // Go-style (no patch) and Python/PEP 440 (with patch).
        assert_eq!(
            parse_prerelease_version("1.24rc1").unwrap(),
            semver::Version::parse("1.24.0-rc.1").unwrap()
        );
        assert_eq!(
            parse_prerelease_version("1.18beta1").unwrap(),
            semver::Version::parse("1.18.0-beta.1").unwrap()
        );
        assert_eq!(
            parse_prerelease_version("3.13.0rc1").unwrap(),
            semver::Version::parse("3.13.0-rc.1").unwrap()
        );
        assert_eq!(
            parse_prerelease_version("3.14.0a1").unwrap(),
            semver::Version::parse("3.14.0-a.1").unwrap()
        );
    }

    #[test]
    fn pads_and_orders_correctly() {
        // Plain numeric versions pad to major.minor.patch, no prerelease.
        assert_eq!(
            parse_prerelease_version("1.24").unwrap(),
            semver::Version::parse("1.24.0").unwrap()
        );
        // Numeric (not lexical) prerelease ordering, and prerelease < release.
        let rc9 = parse_prerelease_version("1.24rc9").unwrap();
        let rc10 = parse_prerelease_version("1.24rc10").unwrap();
        let release = parse_prerelease_version("1.24.0").unwrap();
        assert!(rc9 < rc10);
        assert!(rc9 < release);
    }

    #[test]
    fn rejects_non_prerelease_suffixes_and_junk() {
        // `t` (free-threaded) and `-64` (architecture) are not prereleases.
        assert!(parse_prerelease_version("3.13.2t1").is_none());
        assert!(parse_prerelease_version("3.13.2-64").is_none());
        // A prerelease label without a serial, or with a non-numeric serial, is not a real version.
        assert!(parse_prerelease_version("1.24rc").is_none());
        assert!(parse_prerelease_version("3.14.0a").is_none());
        assert!(parse_prerelease_version("1.24rc1foo").is_none());
        // Too many numeric parts, missing numeric part, and pure junk.
        assert!(parse_prerelease_version("1.2.3.4").is_none());
        assert!(parse_prerelease_version("rc1").is_none());
        assert!(parse_prerelease_version("nonsense").is_none());
    }

    #[test]
    fn default_request_never_reuses_a_prerelease_env() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Python, None, Vec::new(), temp_dir.path())?;
        let default = LanguageRequest::from_config(Language::Python, None).unwrap();

        install_info.with_language_version(semver::Version::parse("3.13.0-rc.1")?);
        assert!(!default.satisfied_by(&install_info));

        // Unless the unqualified request is what selected it: on a machine whose only interpreter
        // is a prerelease, that env is the only one prek can install, so it has to stay reusable.
        install_info.with_extra(UNQUALIFIED_REQUEST_KEY, "1");
        assert!(default.satisfied_by(&install_info));

        install_info.with_language_version(semver::Version::new(3, 13, 0));
        assert!(default.satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn go_default_request_never_reuses_a_prerelease_env() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Golang, None, Vec::new(), temp_dir.path())?;
        let default = LanguageRequest::from_config(Language::Golang, None).unwrap();

        install_info.with_language_version(semver::Version::parse("1.24.0-rc.1")?);
        assert!(!default.satisfied_by(&install_info));

        install_info.with_language_version(semver::Version::new(1, 24, 0));
        assert!(default.satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn go_system_request_stays_permissive_for_prereleases() -> anyhow::Result<()> {
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Golang, None, Vec::new(), temp_dir.path())?;
        install_info.with_language_version(semver::Version::parse("1.24.0-rc.1")?);

        let system = LanguageRequest::parse(Language::Golang, "system").unwrap();
        assert!(system.satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn default_request_stays_permissive_for_other_languages() -> anyhow::Result<()> {
        // Rust's default toolchain can legitimately be nightly or beta (for example when pinned
        // by `rust-toolchain.toml`), unlike Python and Go where a default implies stable.
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Rust, None, Vec::new(), temp_dir.path())?;
        install_info.with_language_version(semver::Version::parse("1.76.0-nightly")?);

        let default = LanguageRequest::from_config(Language::Rust, None).unwrap();
        assert!(default.satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn system_request_stays_permissive_for_prereleases() -> anyhow::Result<()> {
        // `system` pins to whatever is on PATH; a prerelease found there stays reusable, unlike an
        // unqualified default request.
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Python, None, Vec::new(), temp_dir.path())?;
        install_info.with_language_version(semver::Version::parse("3.13.0-rc.1")?);

        let system = LanguageRequest::parse(Language::Python, "system").unwrap();
        assert!(system.satisfied_by(&install_info));

        Ok(())
    }
}
