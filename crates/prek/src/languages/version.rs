use std::str::FromStr;

use crate::config::Language;
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

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) enum LanguageRequest {
    Any { system_only: bool },
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

impl LanguageRequest {
    pub(crate) fn is_any(&self) -> bool {
        match self {
            LanguageRequest::Any { .. } => true,
            LanguageRequest::Bun(req) => req.is_any(),
            LanguageRequest::Dotnet(req) => req.is_any(),
            LanguageRequest::Deno(req) => req.is_any(),
            LanguageRequest::Golang(req) => req.is_any(),
            LanguageRequest::Node(req) => req.is_any(),
            LanguageRequest::Python(req) => req.is_any(),
            LanguageRequest::Ruby(req) => req.is_any(),
            LanguageRequest::Rust(req) => req.is_any(),
            LanguageRequest::Semver(_) => false,
        }
    }

    /// Returns true if this request allows downloading a version.
    ///
    /// Currently, only `system` disallows downloading. In the future,
    /// we may add more specific version requests that also disallow downloading.
    /// For example `language_version: 3.12; system_only`.
    pub(crate) fn allows_download(&self) -> bool {
        match self {
            LanguageRequest::Any { system_only } => !system_only,
            LanguageRequest::Bun(_)
            | LanguageRequest::Dotnet(_)
            | LanguageRequest::Deno(_)
            | LanguageRequest::Golang(_)
            | LanguageRequest::Node(_)
            | LanguageRequest::Python(_)
            | LanguageRequest::Ruby(_)
            | LanguageRequest::Rust(_)
            | LanguageRequest::Semver(_) => true,
        }
    }

    pub(crate) fn parse(lang: Language, request: &str) -> Result<Self, Error> {
        // `pre-commit` support these values in `language_version`:
        // - `default`: substituted by language `get_default_version` function
        //   In `get_default_version`, if a system version is available, it will return `system`.
        //   For Python, it will find from sys.executable, `pythonX.Y`, or versions `py` can find.
        //   Otherwise, it will still return `default`.
        // - `system`: use current system installed version
        // - Python version passed down to `virtualenv`, e.g. `python`, `python3`, `python3.8`
        // - Node.js version passed down to `nodeenv`
        // - Rust version passed down to `rustup`

        if request == "default" || request.is_empty() {
            return Ok(LanguageRequest::Any { system_only: false });
        }
        if request == "system" {
            return Ok(LanguageRequest::Any { system_only: true });
        }

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
            | Language::Perl
            | Language::Php
            | Language::Pygrep
            | Language::R
            | Language::Script
            | Language::Swift
            | Language::System => Self::Semver(request.parse()?),
        })
    }

    pub(crate) fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        match self {
            // A default/omitted `language_version` means a normal, stable interpreter for
            // Python/Go, so it must not silently reuse a prerelease env installed for another
            // hook's explicit request. `system` is exempt: it explicitly pins to whatever is on
            // PATH, prerelease or not, and should stay reusable once installed. Other languages
            // (e.g. Rust, where a nightly/beta toolchain can legitimately be "the default") keep
            // their existing permissive behavior.
            LanguageRequest::Any { system_only } => {
                *system_only
                    || !matches!(install_info.language, Language::Python | Language::Golang)
                    || install_info.language_version.pre.is_empty()
            }
            LanguageRequest::Bun(req) => req.satisfied_by(install_info),
            LanguageRequest::Dotnet(req) => req.satisfied_by(install_info),
            LanguageRequest::Deno(req) => req.satisfied_by(install_info),
            LanguageRequest::Golang(req) => req.satisfied_by(install_info),
            LanguageRequest::Node(req) => req.satisfied_by(install_info),
            LanguageRequest::Python(req) => req.satisfied_by(install_info),
            LanguageRequest::Ruby(req) => req.satisfied_by(install_info),
            LanguageRequest::Rust(req) => req.satisfied_by(install_info),
            LanguageRequest::Semver(req) => req.satisfied_by(install_info),
        }
    }
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub(crate) struct SemverRequest(semver::VersionReq);

impl FromStr for SemverRequest {
    type Err = Error;

    fn from_str(request: &str) -> Result<Self, Self::Err> {
        semver::VersionReq::parse(request)
            .map(SemverRequest)
            .map_err(|_| Error::InvalidVersion(request.to_string()))
    }
}

impl SemverRequest {
    fn satisfied_by(&self, install_info: &InstallInfo) -> bool {
        self.0.matches(&install_info.language_version)
    }
}

pub(crate) fn try_into_u64_slice(version: &str) -> Result<Vec<u64>, std::num::ParseIntError> {
    version
        .split('.')
        .map(str::parse::<u64>)
        .collect::<Result<Vec<_>, _>>()
}

/// Parse a compact prerelease version (Go's `1.24rc1`, PEP 440's `3.13.0rc1`) into
/// semver: pad to `major.minor.patch` and map `rc1` -> `rc.1` so `rc.9` < `rc.10`.
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
    use super::{LanguageRequest, parse_prerelease_version};
    use crate::config::Language;
    use crate::hook::InstallInfo;

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

        let any = LanguageRequest::Any { system_only: false };

        install_info.with_language_version(semver::Version::parse("3.13.0-rc.1")?);
        assert!(!any.satisfied_by(&install_info));

        install_info.with_language_version(semver::Version::new(3, 13, 0));
        assert!(any.satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn default_request_stays_permissive_for_other_languages() -> anyhow::Result<()> {
        // Rust's "default" toolchain can legitimately be nightly/beta (e.g. pinned by
        // `rust-toolchain.toml`), unlike Python/Go where "default" implies stable.
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Rust, None, Vec::new(), temp_dir.path())?;
        install_info.with_language_version(semver::Version::parse("1.76.0-nightly")?);

        let any = LanguageRequest::Any { system_only: false };
        assert!(any.satisfied_by(&install_info));

        Ok(())
    }

    #[test]
    fn system_request_stays_permissive_for_prereleases() -> anyhow::Result<()> {
        // `system` explicitly pins to whatever is on PATH; a prerelease found there should
        // stay reusable, unlike an unqualified default request.
        let temp_dir = tempfile::tempdir()?;
        let mut install_info =
            InstallInfo::create(Language::Python, None, Vec::new(), temp_dir.path())?;
        install_info.with_language_version(semver::Version::parse("3.13.0-rc.1")?);

        let system = LanguageRequest::Any { system_only: true };
        assert!(system.satisfied_by(&install_info));

        Ok(())
    }
}
