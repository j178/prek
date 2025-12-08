use std::ffi::OsString;

use tracing::info;

pub struct EnvVars;

impl EnvVars {
    pub const PATH: &'static str = "PATH";
    pub const HOME: &'static str = "HOME";
    pub const CI: &'static str = "CI";

    // Git related
    pub const GIT_TERMINAL_PROMPT: &'static str = "GIT_TERMINAL_PROMPT";

    pub const SKIP: &'static str = "SKIP";

    // PREK specific environment variables, public for users
    pub const PREK_HOME: &'static str = "PREK_HOME";
    pub const PREK_COLOR: &'static str = "PREK_COLOR";
    pub const PREK_SKIP: &'static str = "PREK_SKIP";
    pub const PREK_ALLOW_NO_CONFIG: &'static str = "PREK_ALLOW_NO_CONFIG";
    pub const PREK_NO_CONCURRENCY: &'static str = "PREK_NO_CONCURRENCY";
    pub const PREK_NO_FAST_PATH: &'static str = "PREK_NO_FAST_PATH";
    pub const PREK_UV_SOURCE: &'static str = "PREK_UV_SOURCE";
    pub const PREK_NATIVE_TLS: &'static str = "PREK_NATIVE_TLS";
    pub const SSL_CERT_FILE: &'static str = "SSL_CERT_FILE";
    pub const PREK_CONTAINER_RUNTIME: &'static str = "PREK_CONTAINER_RUNTIME";

    // PREK internal environment variables
    pub const PREK_INTERNAL__TEST_DIR: &'static str = "PREK_INTERNAL__TEST_DIR";
    pub const PREK_INTERNAL__SORT_FILENAMES: &'static str = "PREK_INTERNAL__SORT_FILENAMES";
    pub const PREK_INTERNAL__SKIP_POST_CHECKOUT: &'static str = "PREK_INTERNAL__SKIP_POST_CHECKOUT";
    pub const PREK_INTERNAL__RUN_ORIGINAL_PRE_COMMIT: &'static str =
        "PREK_INTERNAL__RUN_ORIGINAL_PRE_COMMIT";
    pub const PREK_INTERNAL__GO_BINARY_NAME: &'static str = "PREK_INTERNAL__GO_BINARY_NAME";
    pub const PREK_INTERNAL__NODE_BINARY_NAME: &'static str = "PREK_INTERNAL__NODE_BINARY_NAME";
    pub const PREK_INTERNAL__RUSTUP_BINARY_NAME: &'static str = "PREK_INTERNAL__RUSTUP_BINARY_NAME";
    pub const PREK_GENERATE: &'static str = "PREK_GENERATE";

    // Python & uv related
    pub const VIRTUAL_ENV: &'static str = "VIRTUAL_ENV";
    pub const PYTHONHOME: &'static str = "PYTHONHOME";
    pub const UV_PYTHON: &'static str = "UV_PYTHON";
    pub const UV_CACHE_DIR: &'static str = "UV_CACHE_DIR";
    pub const UV_PYTHON_INSTALL_DIR: &'static str = "UV_PYTHON_INSTALL_DIR";
    pub const UV_MANAGED_PYTHON: &'static str = "UV_MANAGED_PYTHON";
    pub const UV_NO_MANAGED_PYTHON: &'static str = "UV_NO_MANAGED_PYTHON";

    // Node/Npm related
    pub const NPM_CONFIG_USERCONFIG: &'static str = "NPM_CONFIG_USERCONFIG";
    pub const NPM_CONFIG_PREFIX: &'static str = "NPM_CONFIG_PREFIX";
    pub const NODE_PATH: &'static str = "NODE_PATH";

    // Go related
    pub const GOTOOLCHAIN: &'static str = "GOTOOLCHAIN";
    pub const GOROOT: &'static str = "GOROOT";
    pub const GOPATH: &'static str = "GOPATH";
    pub const GOBIN: &'static str = "GOBIN";
    pub const GOFLAGS: &'static str = "GOFLAGS";

    // Lua related
    pub const LUA_PATH: &'static str = "LUA_PATH";
    pub const LUA_CPATH: &'static str = "LUA_CPATH";

    // Ruby related
    pub const GEM_HOME: &'static str = "GEM_HOME";
    pub const GEM_PATH: &'static str = "GEM_PATH";
    pub const BUNDLE_IGNORE_CONFIG: &'static str = "BUNDLE_IGNORE_CONFIG";
    pub const BUNDLE_GEMFILE: &'static str = "BUNDLE_GEMFILE";

    // Rust related
    pub const RUSTUP_TOOLCHAIN: &'static str = "RUSTUP_TOOLCHAIN";
    pub const RUSTUP_AUTO_INSTALL: &'static str = "RUSTUP_AUTO_INSTALL";
    pub const CARGO_HOME: &'static str = "CARGO_HOME";
    pub const RUSTUP_HOME: &'static str = "RUSTUP_HOME";
}

impl EnvVars {
    // Pre-commit environment variables that we support for compatibility
    pub const PRE_COMMIT_HOME: &'static str = "PRE_COMMIT_HOME";
    const PRE_COMMIT_ALLOW_NO_CONFIG: &'static str = "PRE_COMMIT_ALLOW_NO_CONFIG";
    const PRE_COMMIT_NO_CONCURRENCY: &'static str = "PRE_COMMIT_NO_CONCURRENCY";
}

impl EnvVars {
    /// Read an environment variable, falling back to pre-commit corresponding variable if not found.
    pub fn var_os(name: &str) -> Option<OsString> {
        #[allow(clippy::disallowed_methods)]
        std::env::var_os(name).or_else(|| {
            let name = Self::pre_commit_name(name)?;
            let val = std::env::var_os(name)?;
            info!("Falling back to pre-commit environment variable for {name}");
            Some(val)
        })
    }

    pub fn is_set(name: &str) -> bool {
        Self::var_os(name).is_some()
    }

    /// Read an environment variable, falling back to pre-commit corresponding variable if not found.
    pub fn var(name: &str) -> Result<String, std::env::VarError> {
        match Self::var_os(name) {
            Some(s) => s.into_string().map_err(std::env::VarError::NotUnicode),
            None => Err(std::env::VarError::NotPresent),
        }
    }

    /// Read an environment var and parse as bool.
    pub fn var_as_bool(name: &str) -> Option<bool> {
        if let Some(val) = EnvVars::var_os(name)
            && let Some(val) = val.to_str()
            && let Some(val) = EnvVars::parse_boolish(val)
        {
            Some(val)
        } else {
            None
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

    fn pre_commit_name(name: &str) -> Option<&str> {
        match name {
            Self::PREK_ALLOW_NO_CONFIG => Some(Self::PRE_COMMIT_ALLOW_NO_CONFIG),
            Self::PREK_NO_CONCURRENCY => Some(Self::PRE_COMMIT_NO_CONCURRENCY),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::EnvVars;

    #[test]
    fn test_parse_boolish() {
        let true_values = ["y", "yes", "t", "true", "on", "1"];
        let false_values = ["n", "no", "f", "false", "off", "0"];
        for val in true_values {
            assert_eq!(EnvVars::parse_boolish(val), Some(true),);
            assert_eq!(EnvVars::parse_boolish(&val.to_uppercase()), Some(true),);
        }
        for val in false_values {
            assert_eq!(EnvVars::parse_boolish(val), Some(false),);
            assert_eq!(EnvVars::parse_boolish(&val.to_uppercase()), Some(false),);
        }
        assert_eq!(EnvVars::parse_boolish("maybe"), None);
        assert_eq!(EnvVars::parse_boolish(""), None);
        assert_eq!(EnvVars::parse_boolish("123"), None);
    }
}
