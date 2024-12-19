pub struct EnvVars;

impl EnvVars {
    pub const PATH: &'static str = "PATH";

    pub const SKIP: &'static str = "SKIP";

    pub const PREFLIGIT_HOME: &'static str = "PREFLIGIT_HOME";

    // Pre-commit specific environment variables
    pub const PRE_COMMIT_HOME: &'static str = "PRE_COMMIT_HOME";
    pub const PRE_COMMIT_ALLOW_NO_CONFIG: &'static str = "PRE_COMMIT_ALLOW_NO_CONFIG";
    pub const PRE_COMMIT_NO_CONCURRENCY: &'static str = "PRE_COMMIT_NO_CONCURRENCY";
    pub const _PRE_COMMIT_SKIP_POST_CHECKOUT: &'static str = "_PRE_COMMIT_SKIP_POST_CHECKOUT";

    pub const UV_NO_CACHE: &'static str = "UV_NO_CACHE";
    pub const UV_PYTHON_INSTALL_DIR: &'static str = "UV_PYTHON_INSTALL_DIR";
}
