use std::ffi::OsString;

use tracing::info;

pub struct EnvVars;

impl EnvVars {
    pub const PATH: &'static str = "PATH";

    pub const SKIP: &'static str = "SKIP";

    // Prefligit specific environment variables, public for users
    pub const PREFLIGIT_HOME: &'static str = "PREFLIGIT_HOME";
    pub const PREFLIGIT_COLOR: &'static str = "PREFLIGIT_COLOR";
    pub const PREFLIGIT_ALLOW_NO_CONFIG: &'static str = "PREFLIGIT_ALLOW_NO_CONFIG";
    pub const PREFLIGIT_NO_CONCURRENCY: &'static str = "PREFLIGIT_NO_CONCURRENCY";
    pub const PREFLIGIT_NO_FAST_PATH: &'static str = "PREFLIGIT_NO_FAST_PATH";

    // Prefligit internal environment variables
    pub const PREFLIGIT_INTERNAL__TEST_DIR: &'static str = "PREFLIGIT_INTERNAL__TEST_DIR";
    pub const PREFLIGIT_INTERNAL__SORT_FILENAMES: &'static str =
        "PREFLIGIT_INTERNAL__SORT_FILENAMES";
    pub const PREFLIGIT_INTERNAL__SKIP_POST_CHECKOUT: &'static str =
        "PREFLIGIT_INTERNAL__SKIP_POST_CHECKOUT";

    // UV related
    pub const UV_CACHE_DIR: &'static str = "UV_CACHE_DIR";
    pub const UV_PYTHON_INSTALL_DIR: &'static str = "UV_PYTHON_INSTALL_DIR";

    // Node/Npm related
    pub const NPM_CONFIG_USERCONFIG: &'static str = "NPM_CONFIG_USERCONFIG";
    pub const NPM_CONFIG_PREFIX: &'static str = "NPM_CONFIG_PREFIX";
    pub const NODE_PATH: &'static str = "NODE_PATH";

    // Go related
    pub const GOTOOLCHAIN: &'static str = "GOTOOLCHAIN";
    pub const GOROOT: &'static str = "GOROOT";
    pub const GOPATH: &'static str = "GOPATH";
    pub const GOBIN: &'static str = "GOBIN";
}

impl EnvVars {
    // Pre-commit environment variables that we support for compatibility
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

    fn pre_commit_name(name: &str) -> Option<&str> {
        match name {
            Self::PREFLIGIT_ALLOW_NO_CONFIG => Some(Self::PRE_COMMIT_ALLOW_NO_CONFIG),
            Self::PREFLIGIT_NO_CONCURRENCY => Some(Self::PRE_COMMIT_NO_CONCURRENCY),
            _ => None,
        }
    }
}
