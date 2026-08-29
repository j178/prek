#![allow(dead_code, unreachable_pub)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{ChildPath, FileWriteBin, FileWriteStr, PathChild, PathCreateDir};
use etcetera::BaseStrategy;
use rustc_hash::FxHashSet;

use prek_consts::PRE_COMMIT_CONFIG_YAML;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

#[cfg(unix)]
pub fn make_executable(path: impl AsRef<Path>) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let path = path.as_ref();
    let mut permissions = fs_err::metadata(path)?.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs_err::set_permissions(path, permissions)
}

#[cfg(windows)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "Keep the cross-platform test helper API consistent"
)]
pub fn make_executable(_path: impl AsRef<Path>) -> std::io::Result<()> {
    Ok(())
}

fn git_cmd(dir: impl AsRef<Path>, home_dir: impl AsRef<Path>) -> Command {
    let mut cmd = Command::new("git");
    cmd.current_dir(dir)
        .env(EnvVars::PREK_HOME, home_dir.as_ref())
        .args(["-c", "commit.gpgsign=false"])
        .args(["-c", "tag.gpgsign=false"])
        .args(["-c", "core.autocrlf=false"])
        .args(["-c", "user.name=Prek Test"])
        .args(["-c", "user.email=test@prek.dev"]);
    cmd
}

fn write_test_file(root: &ChildPath, file: &Path, content: &[u8]) {
    root.child(file)
        .write_binary(content)
        .unwrap_or_else(|err| panic!("Failed to write test file `{}`: {err}", file.display()));
}

fn write_executable_test_file(root: &ChildPath, file: &Path, content: &[u8]) {
    write_test_file(root, file, content);
    make_executable(root.child(file)).unwrap_or_else(|err| {
        panic!(
            "Failed to make test file `{}` executable: {err}",
            file.display()
        )
    });
}

fn init_repo(path: impl AsRef<Path>, home_dir: impl AsRef<Path>) {
    git_cmd(path, home_dir)
        .arg("-c")
        .arg("init.defaultBranch=master")
        .arg("init")
        .assert()
        .success();
}

/// Git operations for an integration-test repository.
pub struct TestGit<'a> {
    path: PathBuf,
    home_dir: &'a Path,
}

impl<'a> TestGit<'a> {
    fn new(path: impl AsRef<Path>, home_dir: &'a Path) -> Self {
        Self {
            path: path.as_ref().to_path_buf(),
            home_dir,
        }
    }

    /// Create a raw Git command for operations not covered by this wrapper.
    pub fn command(&self) -> Command {
        git_cmd(&self.path, self.home_dir)
    }

    /// Run a Git command that is expected to succeed.
    pub fn run<S>(&self, args: impl IntoIterator<Item = S>) -> &Self
    where
        S: AsRef<OsStr>,
    {
        self.command().args(args).assert().success();
        self
    }

    pub fn init(&self) -> &Self {
        init_repo(&self.path, self.home_dir);
        self
    }

    pub fn add(&self, path: impl AsRef<OsStr>) -> &Self {
        self.command().arg("add").arg(path).assert().success();
        self
    }

    pub fn add_all(&self) -> &Self {
        self.add(".")
    }

    pub fn commit(&self, message: &str) -> &Self {
        self.run(["commit", "-m", message])
    }

    pub fn tag(&self, tag: &str) -> &Self {
        self.command()
            .args(["tag", tag, "-m"])
            .arg(format!("Tag {tag}"))
            .assert()
            .success();
        self
    }

    pub fn rev_parse(&self, rev: &str) -> anyhow::Result<String> {
        let output = self.command().args(["rev-parse", rev]).output()?;
        let output = output.assert().success();
        Ok(std::str::from_utf8(&output.get_output().stdout)?
            .trim()
            .to_owned())
    }

    pub fn rm(&self, path: &str) -> &Self {
        self.run(["rm", "--cached", path]);
        let file_path = self.path.join(path);
        if file_path.exists() {
            fs_err::remove_file(file_path).unwrap();
        }
        self
    }

    pub fn clean(&self) -> &Self {
        self.run(["clean", "-fdx"])
    }

    pub fn branch(&self, branch_name: &str) -> &Self {
        self.run(["branch", branch_name])
    }

    pub fn checkout(&self, branch_name: &str) -> &Self {
        self.run(["checkout", branch_name])
    }
}

pub struct TestRepo {
    path: ChildPath,
    home_dir: PathBuf,
}

impl TestRepo {
    fn new(path: ChildPath, home_dir: PathBuf) -> Self {
        path.create_dir_all()
            .expect("Failed to create test repository directory");
        init_repo(&path, &home_dir);
        Self { path, home_dir }
    }

    pub fn path(&self) -> &ChildPath {
        &self.path
    }

    #[must_use]
    pub fn with_file(self, file: impl AsRef<Path>, content: impl AsRef<[u8]>) -> Self {
        write_test_file(&self.path, file.as_ref(), content.as_ref());
        self
    }

    #[must_use]
    pub fn with_executable_file(self, file: impl AsRef<Path>, content: impl AsRef<[u8]>) -> Self {
        write_executable_test_file(&self.path, file.as_ref(), content.as_ref());
        self
    }

    pub fn git(&self) -> TestGit<'_> {
        TestGit::new(&self.path, &self.home_dir)
    }
}

pub struct TestEnv {
    work_dir: ChildPath,
    home_dir: ChildPath,

    default_filters: OnceLock<Vec<(String, String)>>,
    filters: Vec<(String, String)>,

    // To keep the directory alive.
    _root: tempfile::TempDir,
}

impl TestEnv {
    /// Create an isolated test environment without a Git repository.
    pub fn new() -> Self {
        let bucket = Self::test_bucket_dir();
        fs_err::create_dir_all(&bucket).expect("Failed to create test bucket");

        let root = tempfile::TempDir::new_in(bucket).expect("Failed to create test root directory");

        let work_dir = ChildPath::new(root.path()).child("temp");
        fs_err::create_dir_all(&work_dir).expect("Failed to create test working directory");

        Self::from_root(root, work_dir)
    }

    /// Create an isolated test environment with a Git repository.
    pub fn new_git() -> Self {
        let env = Self::new();
        init_repo(&env.work_dir, &env.home_dir);
        env
    }

    /// Create a Git test environment at the given working directory.
    pub fn new_git_at(path: impl AsRef<Path>) -> Self {
        let env = Self::new_at(path);
        init_repo(&env.work_dir, &env.home_dir);
        env
    }

    fn new_at(path: impl AsRef<Path>) -> Self {
        let bucket = Self::test_bucket_dir();
        fs_err::create_dir_all(&bucket).expect("Failed to create test bucket");

        let root = tempfile::TempDir::new_in(bucket).expect("Failed to create test root directory");

        let work_dir = ChildPath::new(path.as_ref().to_path_buf());
        fs_err::create_dir_all(&work_dir).expect("Failed to create test working directory");

        Self::from_root(root, work_dir)
    }

    fn from_root(root: tempfile::TempDir, work_dir: ChildPath) -> Self {
        let home_dir = ChildPath::new(root.path()).child("home");
        fs_err::create_dir_all(&home_dir).expect("Failed to create test home directory");

        Self {
            work_dir,
            home_dir,
            default_filters: OnceLock::new(),
            filters: Vec::new(),
            _root: root,
        }
    }

    fn build_default_filters(&self) -> Vec<(String, String)> {
        let mut filters = Vec::new();

        filters.extend(
            Self::path_patterns(&self.work_dir)
                .into_iter()
                .map(|pattern| (pattern, "[TEMP_DIR]/".to_string())),
        );
        filters.extend(
            Self::path_patterns(&self.home_dir)
                .into_iter()
                .map(|pattern| (pattern, "[HOME]/".to_string())),
        );
        filters.extend(
            Self::path_patterns(assert_cmd::cargo::cargo_bin!("prek"))
                .into_iter()
                .map(|pattern| (pattern, "[CURRENT_EXE]".to_string())),
        );
        filters.extend(
            INSTA_FILTERS
                .iter()
                .map(|&(matcher, replacement)| (matcher.to_owned(), replacement.to_owned())),
        );

        filters
    }

    fn test_bucket_dir() -> PathBuf {
        EnvVars
            .var(EnvVars::PREK_INTERNAL__TEST_DIR)
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                etcetera::base_strategy::choose_base_strategy()
                    .expect("Failed to find base strategy")
                    .data_dir()
                    .join("prek")
                    .join("tests")
            })
    }

    /// Generate an escaped regex pattern for the given path.
    fn path_pattern(path: impl AsRef<Path>) -> String {
        let separator = r"(\\\\|\\|\/)";
        format!(
            // Trim the trailing separator for cross-platform directories filters
            r"{}{}?",
            regex::escape(&path.as_ref().display().to_string())
                // Make separators platform-agnostic because on Windows we will display
                // paths with Unix-style separators sometimes. `PathBuf` debug output
                // escapes backslash separators, so match those as well.
                .replace('/', separator)
                .replace(r"\\", separator),
            separator,
        )
    }

    /// Generate various escaped regex patterns for the given path.
    fn path_patterns(path: impl AsRef<Path>) -> Vec<String> {
        let mut patterns = Vec::new();

        // We can only canonicalize paths that exist already
        if path.as_ref().exists() {
            patterns.push(Self::path_pattern(
                path.as_ref()
                    .canonicalize()
                    .expect("Failed to create canonical path"),
            ));
        }

        // Include a non-canonicalized version
        patterns.push(Self::path_pattern(path));

        patterns
    }

    /// Read a file in the temporary directory
    pub fn read(&self, file: impl AsRef<Path>) -> String {
        fs_err::read_to_string(self.work_dir.join(&file))
            .unwrap_or_else(|_| panic!("Missing file: `{}`", file.as_ref().display()))
    }

    /// Write or replace a file in the working directory.
    pub fn write_file(&self, file: impl AsRef<Path>, content: impl AsRef<[u8]>) {
        write_test_file(&self.work_dir, file.as_ref(), content.as_ref());
    }

    /// Write a file in the working directory and return this environment.
    #[must_use]
    pub fn with_file(self, file: impl AsRef<Path>, content: impl AsRef<[u8]>) -> Self {
        self.write_file(file, content);
        self
    }

    /// Write files in the working directory and return this environment.
    #[must_use]
    pub fn with_files<P, C>(self, files: impl IntoIterator<Item = (P, C)>) -> Self
    where
        P: AsRef<Path>,
        C: AsRef<[u8]>,
    {
        for (file, content) in files {
            self.write_file(file, content);
        }
        self
    }

    /// Write or replace an executable file in the working directory.
    pub fn write_executable_file(&self, file: impl AsRef<Path>, content: impl AsRef<[u8]>) {
        write_executable_test_file(&self.work_dir, file.as_ref(), content.as_ref());
    }

    /// Write an executable file in the working directory and return this environment.
    #[must_use]
    pub fn with_executable_file(self, file: impl AsRef<Path>, content: impl AsRef<[u8]>) -> Self {
        self.write_executable_file(file, content);
        self
    }

    pub fn command(&self) -> Command {
        let mut cmd = Command::new(assert_cmd::cargo::cargo_bin!("prek"));
        cmd.current_dir(self.work_dir())
            .env(EnvVars::PREK_HOME, &**self.home_dir())
            .env(EnvVars::PREK_INTERNAL__SORT_FILENAMES, "1")
            // Git commands spawned by prek do not inherit `git_cmd`'s `-c` arguments.
            .envs([
                ("GIT_CONFIG_COUNT", "1"),
                ("GIT_CONFIG_KEY_0", "core.autocrlf"),
                ("GIT_CONFIG_VALUE_0", "false"),
            ])
            .env(
                EnvVars::PREK_INTERNAL__USER_CONFIG_PATH,
                self.user_config_path().path(),
            )
            .env_remove("RUST_LOG");

        cmd
    }

    pub fn git(&self) -> TestGit<'_> {
        self.git_at(&self.work_dir)
    }

    pub fn git_at(&self, dir: impl AsRef<Path>) -> TestGit<'_> {
        TestGit::new(dir, self.home_dir.as_ref())
    }

    fn user_config_path(&self) -> ChildPath {
        self.home_dir
            .child("config")
            .child("prek")
            .child("prek.toml")
    }

    pub fn write_user_config(&self, content: &str) {
        let config_dir = self.home_dir.child("config").child("prek");
        config_dir
            .create_dir_all()
            .expect("Failed to create user config directory");
        self.user_config_path()
            .write_str(content)
            .expect("Failed to write user config");
    }

    pub fn run(&self) -> Command {
        self.subcommand("run")
    }

    pub fn exec(&self) -> Command {
        self.subcommand("exec")
    }

    pub fn validate_config(&self) -> Command {
        self.subcommand("validate-config")
    }

    pub fn validate_manifest(&self) -> Command {
        self.subcommand("validate-manifest")
    }

    pub fn install(&self) -> Command {
        self.subcommand("install")
    }

    pub fn prepare_hooks(&self) -> Command {
        self.subcommand("prepare-hooks")
    }

    pub fn uninstall(&self) -> Command {
        self.subcommand("uninstall")
    }

    pub fn sample_config(&self) -> Command {
        self.subcommand("sample-config")
    }

    pub fn list(&self) -> Command {
        self.subcommand("list")
    }

    pub fn update(&self) -> Command {
        self.subcommand("update")
    }

    pub fn try_repo(&self) -> Command {
        self.subcommand("try-repo")
    }

    fn subcommand(&self, name: &str) -> Command {
        let mut command = self.command();
        command.arg(name);
        command
    }

    #[must_use]
    pub fn with_filter(
        mut self,
        matcher: impl Into<String>,
        replacement: impl Into<String>,
    ) -> Self {
        self.filters.push((matcher.into(), replacement.into()));
        self
    }

    #[must_use]
    pub fn with_filters<M, R>(mut self, filters: impl IntoIterator<Item = (M, R)>) -> Self
    where
        M: Into<String>,
        R: Into<String>,
    {
        self.filters.extend(
            filters
                .into_iter()
                .map(|(matcher, replacement)| (matcher.into(), replacement.into())),
        );
        self
    }

    pub fn with_snapshot_settings<T>(&self, f: impl FnOnce() -> T) -> T {
        let default_filters = self
            .default_filters
            .get_or_init(|| self.build_default_filters());
        bind_filters(
            default_filters
                .iter()
                .chain(&self.filters)
                .map(|(matcher, replacement)| (matcher.as_str(), replacement.as_str())),
            f,
        )
    }

    /// Get the working directory for the test environment.
    pub fn work_dir(&self) -> &ChildPath {
        &self.work_dir
    }

    /// Get a path relative to the working directory.
    pub fn child(&self, path: impl AsRef<Path>) -> ChildPath {
        self.work_dir.child(path)
    }

    /// Get the home directory for the test environment.
    pub fn home_dir(&self) -> &ChildPath {
        &self.home_dir
    }

    /// Create a Git repository that can be used as a local remote.
    pub fn create_repo(&self, name: impl AsRef<Path>) -> TestRepo {
        TestRepo::new(
            self.home_dir.child("test-repos").child(name),
            self.home_dir.to_path_buf(),
        )
    }

    /// Write a `.pre-commit-config.yaml` file and return this environment.
    #[must_use]
    pub fn with_config(self, content: impl AsRef<str>) -> Self {
        self.write_config(content);
        self
    }

    /// Write or replace the `.pre-commit-config.yaml` file in the working directory.
    pub fn write_config(&self, content: impl AsRef<str>) {
        self.work_dir
            .child(PRE_COMMIT_CONFIG_YAML)
            .write_str(content.as_ref())
            .expect("Failed to write pre-commit config");
    }

    /// Write a `.pre-commit-config.yaml` file for a nested project.
    fn write_project_config(&self, project: impl AsRef<Path>, content: impl AsRef<str>) {
        self.write_file(
            project.as_ref().join(PRE_COMMIT_CONFIG_YAML),
            content.as_ref(),
        );
    }

    /// Write a nested project config and return this environment.
    #[must_use]
    pub fn with_project_config(self, project: impl AsRef<Path>, content: impl AsRef<str>) -> Self {
        self.write_project_config(project, content);
        self
    }

    /// Write the same config for the workspace root and each nested project.
    pub fn write_workspace<P>(
        &self,
        project_paths: impl IntoIterator<Item = P>,
        config: impl AsRef<str>,
    ) where
        P: AsRef<Path>,
    {
        let config = config.as_ref();
        self.write_config(config);

        for path in project_paths {
            self.write_project_config(path, config);
        }
    }

    /// Write workspace configs and return this environment.
    #[must_use]
    pub fn with_workspace<P>(
        self,
        project_paths: impl IntoIterator<Item = P>,
        config: impl AsRef<str>,
    ) -> Self
    where
        P: AsRef<Path>,
    {
        self.write_workspace(project_paths, config);
        self
    }
}

#[doc(hidden)]
pub fn bind_filters<'a, T>(
    filters: impl IntoIterator<Item = (&'a str, &'a str)>,
    f: impl FnOnce() -> T,
) -> T {
    let mut settings = insta::Settings::clone_current();
    for (matcher, replacement) in filters {
        settings.add_filter(matcher, replacement);
    }
    settings.bind(f)
}

#[doc(hidden)] // Macro and test environment only, don't use directly.
pub const INSTA_FILTERS: &[(&str, &str)] = &[
    // File sizes
    (r"(\s|\()(\d+\.)?\d+\s?([KMGTPE]i)?B", "$1[SIZE]"),
    // Rewrite Windows output to Unix output
    (r"\\{1,2}([\w\d]|\.\.|\.)", "/$1"),
    // Process exit status wording differs on Windows
    (r"(?m)^exit code: ", "exit status: "),
    // Non-deterministic stash patch names
    (r"/\d+-\d+\.patch", "/[TIME]-[PID].patch"),
    // Non-deterministic Git commit summaries
    (r"\[master [0-9a-f]{7}\]", "[master COMMIT]"),
    // The exact message is host language dependent
    (
        r"Caused by: .* \(os error 2\)",
        "Caused by: No such file or directory (os error 2)",
    ),
    // Time seconds
    (r"\b(\d+\.)?\d+(ms|s)\b", "[TIME]"),
    // Strip non-deterministic lock contention warnings from parallel test execution
    (r"(?m)^warning: Waiting to acquire lock.*\n", ""),
];

#[allow(unused_macros)]
macro_rules! cmd_snapshot {
    ($spawnable:expr, @$snapshot:literal) => {{
        $crate::common::bind_filters(
            $crate::common::INSTA_FILTERS.iter().copied(),
            || insta_cmd::assert_cmd_snapshot!($spawnable, @$snapshot),
        )
    }};
    ($context:expr, $spawnable:expr, @$snapshot:literal) => {{
        $context.with_snapshot_settings(|| {
            insta_cmd::assert_cmd_snapshot!($spawnable, @$snapshot);
        });
    }};
}

#[allow(unused_imports)]
pub(crate) use cmd_snapshot;

#[allow(unused_macros)]
macro_rules! snapshot {
    ($context:expr, $value:expr, @$snapshot:literal) => {{
        $context.with_snapshot_settings(|| {
            insta::assert_snapshot!($value, @$snapshot);
        });
    }};
}

#[allow(unused_imports)]
pub(crate) use snapshot;

pub(crate) fn remove_bin_from_path(bin: &str, path: Option<OsString>) -> anyhow::Result<OsString> {
    let path = path.unwrap_or(EnvVars.var_os(EnvVars::PATH).expect("Path must be set"));
    let Ok(dirs) = which::which_all(bin) else {
        return Ok(path);
    };

    let dirs: FxHashSet<_> = dirs
        .filter_map(|path| path.parent().map(Path::to_path_buf))
        .collect();

    let new_path_entries: Vec<_> = std::env::split_paths(&path)
        .filter(|path| !dirs.contains(path.as_path()))
        .collect();

    Ok(std::env::join_paths(new_path_entries)?)
}
