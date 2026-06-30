use std::ffi::OsString;

use tracing::info;

pub struct EnvVars;

pub trait EnvVarsRead {
    fn var_os(&self, name: &str) -> Option<OsString>;

    fn is_set(&self, name: &str) -> bool {
        self.var_os(name).is_some()
    }

    fn var(&self, name: &str) -> Result<String, std::env::VarError> {
        match self.var_os(name) {
            Some(s) => s.into_string().map_err(std::env::VarError::NotUnicode),
            None => Err(std::env::VarError::NotPresent),
        }
    }

    fn var_as_bool(&self, name: &str) -> Result<Option<bool>, String> {
        let Some(val) = self.var_os(name) else {
            return Ok(None);
        };
        let val = val.to_string_lossy();
        parse_boolish(&val)
            .map(Some)
            .ok_or_else(|| val.into_owned())
    }
}

impl EnvVarsRead for EnvVars {
    fn var_os(&self, name: &str) -> Option<OsString> {
        #[allow(clippy::disallowed_methods)]
        std::env::var_os(name).or_else(|| {
            let name = EnvVars::pre_commit_name(name)?;
            let val = std::env::var_os(name)?;
            info!("Falling back to pre-commit environment variable for {name}");
            Some(val)
        })
    }
}

#[cfg(any(test, feature = "test-utils"))]
#[derive(Debug, Clone, Copy)]
pub struct EnvVarsMap<'a> {
    values: &'a [(&'a str, &'a str)],
}

#[cfg(any(test, feature = "test-utils"))]
impl EnvVarsMap<'_> {
    fn direct_var_os(&self, name: &str) -> Option<OsString> {
        self.values
            .iter()
            .find_map(|(key, value)| (*key == name).then(|| OsString::from(value)))
    }
}

#[cfg(any(test, feature = "test-utils"))]
impl EnvVarsRead for EnvVarsMap<'_> {
    fn var_os(&self, name: &str) -> Option<OsString> {
        self.direct_var_os(name).or_else(|| {
            let name = EnvVars::pre_commit_name(name)?;
            let val = self.direct_var_os(name)?;
            info!("Falling back to pre-commit environment variable for {name}");
            Some(val)
        })
    }
}

impl EnvVars {
    pub const PATH: &'static str = "PATH";
    pub const HOME: &'static str = "HOME";
    pub const CI: &'static str = "CI";
    pub const LC_ALL: &'static str = "LC_ALL";

    // Git related
    pub const GIT_DIR: &'static str = "GIT_DIR";
    pub const GIT_WORK_TREE: &'static str = "GIT_WORK_TREE";
    pub const GIT_TERMINAL_PROMPT: &'static str = "GIT_TERMINAL_PROMPT";

    pub const SKIP: &'static str = "SKIP";

    // PREK specific environment variables, public for users
    pub const PREK_HOME: &'static str = "PREK_HOME";
    pub const PREK_COLOR: &'static str = "PREK_COLOR";
    pub const PREK_SKIP: &'static str = "PREK_SKIP";
    pub const PREK_ALLOW_NO_CONFIG: &'static str = "PREK_ALLOW_NO_CONFIG";
    pub const PREK_NO_CONCURRENCY: &'static str = "PREK_NO_CONCURRENCY";
    pub const PREK_CONCURRENT_HOOKS: &'static str = "PREK_CONCURRENT_HOOKS";
    pub const PREK_CONCURRENT_BATCHES: &'static str = "PREK_CONCURRENT_BATCHES";
    pub const PREK_MAX_CONCURRENCY: &'static str = "PREK_MAX_CONCURRENCY";
    pub const PREK_NO_FAST_PATH: &'static str = "PREK_NO_FAST_PATH";
    pub const PREK_UV_SOURCE: &'static str = "PREK_UV_SOURCE";
    pub const PREK_NATIVE_TLS: &'static str = "PREK_NATIVE_TLS";
    pub const PREK_DOWNLOAD_CHECKSUM_POLICY: &'static str = "PREK_DOWNLOAD_CHECKSUM_POLICY";
    pub const SSL_CERT_FILE: &'static str = "SSL_CERT_FILE";
    pub const SSL_CERT_DIR: &'static str = "SSL_CERT_DIR";
    pub const PREK_CONTAINER_RUNTIME: &'static str = "PREK_CONTAINER_RUNTIME";
    pub const PREK_DOCKER_NO_INIT: &'static str = "PREK_DOCKER_NO_INIT";
    pub const PREK_QUIET: &'static str = "PREK_QUIET";

    // PREK internal environment variables
    pub const PREK_INTERNAL__TEST_DIR: &'static str = "PREK_INTERNAL__TEST_DIR";
    pub const PREK_INTERNAL__USER_CONFIG_PATH: &'static str = "PREK_INTERNAL__USER_CONFIG_PATH";
    pub const PREK_INTERNAL__SORT_FILENAMES: &'static str = "PREK_INTERNAL__SORT_FILENAMES";
    pub const PREK_INTERNAL__SKIP_POST_CHECKOUT: &'static str = "PREK_INTERNAL__SKIP_POST_CHECKOUT";
    pub const PREK_INTERNAL__RUN_ORIGINAL_PRE_COMMIT: &'static str =
        "PREK_INTERNAL__RUN_ORIGINAL_PRE_COMMIT";
    pub const PREK_INTERNAL__BUN_BINARY_NAME: &'static str = "PREK_INTERNAL__BUN_BINARY_NAME";
    pub const PREK_INTERNAL__DENO_BINARY_NAME: &'static str = "PREK_INTERNAL__DENO_BINARY_NAME";
    pub const PREK_INTERNAL__DOTNET_BINARY_NAME: &'static str = "PREK_INTERNAL_DOTNET_BINARY_NAME";
    pub const PREK_INTERNAL__GO_BINARY_NAME: &'static str = "PREK_INTERNAL__GO_BINARY_NAME";
    pub const PREK_INTERNAL__NODE_BINARY_NAME: &'static str = "PREK_INTERNAL__NODE_BINARY_NAME";
    pub const PREK_INTERNAL__RUSTUP_BINARY_NAME: &'static str = "PREK_INTERNAL__RUSTUP_BINARY_NAME";
    pub const PREK_INTERNAL__SKIP_CABAL_UPDATE: &'static str = "PREK_INTERNAL__SKIP_CABAL_UPDATE";
    pub const PREK_RUNNING_LEGACY: &'static str = "PREK_RUNNING_LEGACY";
    pub const PREK_GENERATE: &'static str = "PREK_GENERATE";

    // Python & uv related
    pub const VIRTUAL_ENV: &'static str = "VIRTUAL_ENV";
    pub const PYTHONHOME: &'static str = "PYTHONHOME";
    pub const UV_PYTHON: &'static str = "UV_PYTHON";
    pub const UV_SYSTEM_PYTHON: &'static str = "UV_SYSTEM_PYTHON";
    pub const UV_CACHE_DIR: &'static str = "UV_CACHE_DIR";
    pub const UV_PYTHON_INSTALL_DIR: &'static str = "UV_PYTHON_INSTALL_DIR";
    pub const UV_MANAGED_PYTHON: &'static str = "UV_MANAGED_PYTHON";
    pub const UV_NO_MANAGED_PYTHON: &'static str = "UV_NO_MANAGED_PYTHON";

    // Node/Npm related
    pub const NODE_PATH: &'static str = "NODE_PATH";

    // Bun related
    pub const BUN_INSTALL: &'static str = "BUN_INSTALL";

    // Deno related
    pub const DENO_DIR: &'static str = "DENO_DIR";
    pub const DENO_NO_UPDATE_CHECK: &'static str = "DENO_NO_UPDATE_CHECK";

    // GitHub API authentication (to avoid rate limits)
    pub const GITHUB_TOKEN: &'static str = "GITHUB_TOKEN";

    // Go related
    pub const GOTOOLCHAIN: &'static str = "GOTOOLCHAIN";
    pub const GOROOT: &'static str = "GOROOT";
    pub const GOPATH: &'static str = "GOPATH";
    pub const GOBIN: &'static str = "GOBIN";
    pub const GOFLAGS: &'static str = "GOFLAGS";

    // Lua related
    pub const LUA_PATH: &'static str = "LUA_PATH";
    pub const LUA_CPATH: &'static str = "LUA_CPATH";

    // Perl related
    pub const PERL5LIB: &'static str = "PERL5LIB";
    pub const PERL_MB_OPT: &'static str = "PERL_MB_OPT";
    pub const PERL_MM_OPT: &'static str = "PERL_MM_OPT";

    // R related
    pub const R_HOME: &'static str = "R_HOME";
    pub const R_PROFILE_USER: &'static str = "R_PROFILE_USER";
    pub const RENV_PROJECT: &'static str = "RENV_PROJECT";

    // Conda related
    pub const CONDA_PREFIX: &'static str = "CONDA_PREFIX";
    pub const PRE_COMMIT_USE_MAMBA: &'static str = "PRE_COMMIT_USE_MAMBA";
    pub const PRE_COMMIT_USE_MICROMAMBA: &'static str = "PRE_COMMIT_USE_MICROMAMBA";

    // Dart related
    pub const PUB_CACHE: &'static str = "PUB_CACHE";

    // Coursier related
    pub const COURSIER_CACHE: &'static str = "COURSIER_CACHE";

    // Ruby related
    pub const PREK_RUBY_MIRROR: &'static str = "PREK_RUBY_MIRROR";
    pub const GEM_HOME: &'static str = "GEM_HOME";
    pub const GEM_PATH: &'static str = "GEM_PATH";
    pub const BUNDLE_IGNORE_CONFIG: &'static str = "BUNDLE_IGNORE_CONFIG";
    pub const BUNDLE_GEMFILE: &'static str = "BUNDLE_GEMFILE";

    // Rust related
    pub const PREK_RUST_PROFILE: &'static str = "PREK_RUST_PROFILE";
    pub const RUSTUP_TOOLCHAIN: &'static str = "RUSTUP_TOOLCHAIN";
    pub const RUSTUP_AUTO_INSTALL: &'static str = "RUSTUP_AUTO_INSTALL";
    pub const CARGO_HOME: &'static str = "CARGO_HOME";
    pub const RUSTUP_HOME: &'static str = "RUSTUP_HOME";

    // .NET related
    pub const DOTNET_ROOT: &'static str = "DOTNET_ROOT";
}

#[cfg(any(test, feature = "test-utils"))]
impl EnvVars {
    pub const fn from_map<'a>(values: &'a [(&'a str, &'a str)]) -> EnvVarsMap<'a> {
        EnvVarsMap { values }
    }
}

impl EnvVars {
    // Pre-commit environment variables that we support for compatibility
    pub const PRE_COMMIT_HOME: &'static str = "PRE_COMMIT_HOME";
    const PRE_COMMIT_ALLOW_NO_CONFIG: &'static str = "PRE_COMMIT_ALLOW_NO_CONFIG";
    const PRE_COMMIT_NO_CONCURRENCY: &'static str = "PRE_COMMIT_NO_CONCURRENCY";
}

impl EnvVars {
    /// Return whether the current process is running under CI.
    pub fn is_under_ci() -> bool {
        EnvVars.is_set(Self::CI)
    }

    fn pre_commit_name(name: &str) -> Option<&str> {
        match name {
            Self::PREK_ALLOW_NO_CONFIG => Some(Self::PRE_COMMIT_ALLOW_NO_CONFIG),
            Self::PREK_NO_CONCURRENCY => Some(Self::PRE_COMMIT_NO_CONCURRENCY),
            _ => None,
        }
    }
}

/// Parse a boolean from a string.
///
/// Adapted from Clap's `BoolishValueParser` which is dual licensed under the MIT and Apache-2.0.
/// See `clap_builder/src/util/str_to_bool.rs`
fn parse_boolish(val: &str) -> Option<bool> {
    // True values are `y`, `yes`, `t`, `true`, `on`, and `1`.
    const TRUE_LITERALS: [&str; 6] = ["y", "yes", "t", "true", "on", "1"];

    // False values are `n`, `no`, `f`, `false`, `off`, and `0`.
    const FALSE_LITERALS: [&str; 6] = ["n", "no", "f", "false", "off", "0"];

    let val = val.to_lowercase();
    let pat = val.as_str();
    if TRUE_LITERALS.contains(&pat) {
        Some(true)
    } else if FALSE_LITERALS.contains(&pat) {
        Some(false)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::{EnvVars, EnvVarsRead, parse_boolish};

    #[test]
    fn test_parse_boolish() {
        let true_values = ["y", "yes", "t", "true", "on", "1"];
        let false_values = ["n", "no", "f", "false", "off", "0"];
        for val in true_values {
            assert_eq!(parse_boolish(val), Some(true),);
            assert_eq!(parse_boolish(&val.to_uppercase()), Some(true),);
        }
        for val in false_values {
            assert_eq!(parse_boolish(val), Some(false),);
            assert_eq!(parse_boolish(&val.to_uppercase()), Some(false),);
        }
        assert_eq!(parse_boolish("maybe"), None);
        assert_eq!(parse_boolish(""), None);
        assert_eq!(parse_boolish("123"), None);
    }

    #[test]
    fn test_env_vars_read_helpers() {
        let env_vars = EnvVars::from_map(&[(EnvVars::PREK_COLOR, "never")]);
        assert_eq!(env_vars.var(EnvVars::PREK_COLOR).unwrap(), "never");
        assert!(env_vars.is_set(EnvVars::PREK_COLOR));

        let env_vars = EnvVars::from_map(&[]);
        assert!(matches!(
            env_vars.var(EnvVars::PREK_COLOR),
            Err(std::env::VarError::NotPresent)
        ));
        assert!(!env_vars.is_set(EnvVars::PREK_COLOR));

        let env_vars = EnvVars::from_map(&[(EnvVars::PRE_COMMIT_NO_CONCURRENCY, "1")]);
        assert_eq!(env_vars.var(EnvVars::PREK_NO_CONCURRENCY).unwrap(), "1");

        let env_vars = EnvVars::from_map(&[
            (EnvVars::PREK_NO_CONCURRENCY, "prek"),
            (EnvVars::PRE_COMMIT_NO_CONCURRENCY, "pre-commit"),
        ]);
        assert_eq!(env_vars.var(EnvVars::PREK_NO_CONCURRENCY).unwrap(), "prek");

        let env_vars = EnvVars::from_map(&[
            (EnvVars::PREK_DOCKER_NO_INIT, "yes"),
            (EnvVars::PREK_NATIVE_TLS, "0"),
            (EnvVars::PREK_ALLOW_NO_CONFIG, "maybe"),
        ]);
        assert_eq!(
            env_vars.var_as_bool(EnvVars::PREK_DOCKER_NO_INIT),
            Ok(Some(true))
        );
        assert_eq!(
            env_vars.var_as_bool(EnvVars::PREK_NATIVE_TLS),
            Ok(Some(false))
        );
        assert_eq!(
            env_vars.var_as_bool(EnvVars::PREK_ALLOW_NO_CONFIG),
            Err("maybe".to_string())
        );
    }
}
