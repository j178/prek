#![allow(dead_code, unreachable_pub)]

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;

use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{ChildPath, FileWriteStr, PathChild, PathCreateDir};
use etcetera::BaseStrategy;
use rustc_hash::FxHashSet;

use prek_consts::PRE_COMMIT_CONFIG_YAML;
use prek_consts::env_vars::EnvVars;

pub struct TestContext {
    temp_dir: ChildPath,
    home_dir: ChildPath,

    /// Standard filters for this test context.
    filters: Vec<(String, String)>,

    // To keep the directory alive.
    #[allow(dead_code)]
    _root: tempfile::TempDir,
}

impl TestContext {
    pub fn new() -> Self {
        let bucket = Self::test_bucket_dir();
        fs_err::create_dir_all(&bucket).expect("Failed to create test bucket");

        let root = tempfile::TempDir::new_in(bucket).expect("Failed to create test root directory");

        let temp_dir = ChildPath::new(root.path()).child("temp");
        fs_err::create_dir_all(&temp_dir).expect("Failed to create test working directory");

        Self::from_root(root, temp_dir)
    }

    pub fn new_at(path: PathBuf) -> Self {
        let bucket = Self::test_bucket_dir();
        fs_err::create_dir_all(&bucket).expect("Failed to create test bucket");

        let root = tempfile::TempDir::new_in(bucket).expect("Failed to create test root directory");

        let temp_dir = ChildPath::new(path);
        fs_err::create_dir_all(&temp_dir).expect("Failed to create test working directory");

        Self::from_root(root, temp_dir)
    }

    fn from_root(root: tempfile::TempDir, temp_dir: ChildPath) -> Self {
        let home_dir = ChildPath::new(root.path()).child("home");
        fs_err::create_dir_all(&home_dir).expect("Failed to create test home directory");

        let mut filters = Vec::new();

        filters.extend(
            Self::path_patterns(&temp_dir)
                .into_iter()
                .map(|pattern| (pattern, "[TEMP_DIR]/".to_string())),
        );
        filters.extend(
            Self::path_patterns(&home_dir)
                .into_iter()
                .map(|pattern| (pattern, "[HOME]/".to_string())),
        );

        let current_exe = assert_cmd::cargo::cargo_bin!("prek");
        filters.extend(
            Self::path_patterns(current_exe)
                .into_iter()
                .map(|pattern| (pattern, "[CURRENT_EXE]".to_string())),
        );

        Self {
            temp_dir,
            home_dir,
            filters,
            _root: root,
        }
    }

    pub fn test_bucket_dir() -> PathBuf {
        EnvVars::var(EnvVars::PREK_INTERNAL__TEST_DIR)
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
        format!(
            // Trim the trailing separator for cross-platform directories filters
            r"{}\\?/?",
            regex::escape(&path.as_ref().display().to_string())
                // Make separators platform agnostic because on Windows we will display
                // paths with Unix-style separators sometimes
                .replace(r"\\", r"(\\|\/)")
        )
    }

    /// Generate various escaped regex patterns for the given path.
    pub fn path_patterns(path: impl AsRef<Path>) -> Vec<String> {
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
        fs_err::read_to_string(self.temp_dir.join(&file))
            .unwrap_or_else(|_| panic!("Missing file: `{}`", file.as_ref().display()))
    }

    pub fn command(&self) -> Command {
        if EnvVars::is_set(EnvVars::PREK_INTERNAL__RUN_ORIGINAL_PRE_COMMIT) {
            // Run the original pre-commit to check compatibility.
            let mut cmd = Command::new("pre-commit");
            cmd.current_dir(self.work_dir());
            cmd.env(EnvVars::PRE_COMMIT_HOME, &**self.home_dir());
            cmd
        } else {
            let bin = assert_cmd::cargo::cargo_bin!("prek");
            let mut cmd = Command::new(bin);
            cmd.current_dir(self.work_dir());
            cmd.env(EnvVars::PREK_HOME, &**self.home_dir());
            cmd.env(EnvVars::PREK_INTERNAL__SORT_FILENAMES, "1");
            cmd
        }
    }

    pub fn run(&self) -> Command {
        let mut command = self.command();
        command.arg("run");
        command
    }

    pub fn validate_config(&self) -> Command {
        let mut command = self.command();
        command.arg("validate-config");
        command
    }

    pub fn validate_manifest(&self) -> Command {
        let mut command = self.command();
        command.arg("validate-manifest");
        command
    }

    pub fn install(&self) -> Command {
        let mut command = self.command();
        command.arg("install");
        command
    }

    pub fn install_hooks(&self) -> Command {
        let mut command = self.command();
        command.arg("install-hooks");
        command
    }

    pub fn uninstall(&self) -> Command {
        let mut command = self.command();
        command.arg("uninstall");
        command
    }

    pub fn sample_config(&self) -> Command {
        let mut command = self.command();
        command.arg("sample-config");
        command
    }

    pub fn list(&self) -> Command {
        let mut command = self.command();
        command.arg("list");
        command
    }

    pub fn auto_update(&self) -> Command {
        let mut cmd = self.command();
        cmd.arg("auto-update");
        cmd
    }

    pub fn try_repo(&self) -> Command {
        let mut cmd = self.command();
        cmd.arg("try-repo");
        cmd
    }

    /// Standard snapshot filters _plus_ those for this test context.
    pub fn filters(&self) -> Vec<(&str, &str)> {
        // Put test context snapshots before the default filters
        // This ensures we don't replace other patterns inside paths from the test context first
        self.filters
            .iter()
            .map(|(p, r)| (p.as_str(), r.as_str()))
            .chain(INSTA_FILTERS.iter().copied())
            .collect()
    }

    /// Get the working directory for the test context.
    pub fn work_dir(&self) -> &ChildPath {
        &self.temp_dir
    }

    /// Get the home directory for the test context.
    pub fn home_dir(&self) -> &ChildPath {
        &self.home_dir
    }

    /// Initialize a sample project for prek.
    pub fn init_project(&self) {
        Command::new("git")
            .arg("-c")
            .arg("init.defaultBranch=master")
            .arg("init")
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Configure git user and email.
    pub fn configure_git_author(&self) {
        Command::new("git")
            .arg("config")
            .arg("user.name")
            .arg("Prek Test")
            .current_dir(&self.temp_dir)
            .assert()
            .success();
        Command::new("git")
            .arg("config")
            .arg("user.email")
            .arg("test@prek.dev")
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    pub fn disable_auto_crlf(&self) {
        // Disable autocrlf
        Command::new("git")
            .arg("config")
            .arg("core.autocrlf")
            .arg("false")
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Run `git add`.
    pub fn git_add(&self, path: impl AsRef<OsStr>) {
        Command::new("git")
            .arg("add")
            .arg(path)
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Run `git commit`.
    pub fn git_commit(&self, message: &str) {
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg(message)
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Run `git tag`.
    pub fn git_tag(&self, tag: &str) {
        Command::new("git")
            .arg("tag")
            .arg(tag)
            .arg("-m")
            .arg(format!("Tag {tag}"))
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Run `git reset`.
    pub fn git_reset(&self, target: &str) {
        Command::new("git")
            .arg("reset")
            .arg(target)
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Run `git rm`.
    pub fn git_rm(&self, path: &str) {
        Command::new("git")
            .arg("rm")
            .arg("--cached")
            .arg(path)
            .current_dir(&self.temp_dir)
            .assert()
            .success();
        let file_path = self.temp_dir.child(path);
        if file_path.exists() {
            fs_err::remove_file(file_path).unwrap();
        }
    }

    /// Run `git clean`.
    pub fn git_clean(&self) {
        Command::new("git")
            .arg("clean")
            .arg("-fdx")
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Create a new git branch.
    pub fn git_branch(&self, branch_name: &str) {
        Command::new("git")
            .arg("branch")
            .arg(branch_name)
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Switch to a git branch.
    pub fn git_checkout(&self, branch_name: &str) {
        Command::new("git")
            .arg("checkout")
            .arg(branch_name)
            .current_dir(&self.temp_dir)
            .assert()
            .success();
    }

    /// Write a `.pre-commit-config.yaml` file in the temporary directory.
    pub fn write_pre_commit_config(&self, content: &str) {
        self.temp_dir
            .child(PRE_COMMIT_CONFIG_YAML)
            .write_str(content)
            .expect("Failed to write pre-commit config");
    }

    /// Setup a workspace with multiple projects, each with the same config.
    /// This creates a tree-like directory structure for testing workspace functionality.
    pub fn setup_workspace(&self, project_paths: &[&str], config: &str) -> anyhow::Result<()> {
        // Always create root config
        self.temp_dir
            .child(PRE_COMMIT_CONFIG_YAML)
            .write_str(config)?;

        // Create each project directory and config
        for path in project_paths {
            let project_dir = self.temp_dir.child(path);
            project_dir.create_dir_all()?;
            project_dir
                .child(PRE_COMMIT_CONFIG_YAML)
                .write_str(config)?;
        }

        Ok(())
    }

    /// Add extra filtering for cache size output
    #[must_use]
    pub fn with_filtered_cache_size(mut self) -> Self {
        // Filter raw byte counts (numbers on their own line)
        self.filters
            .push((r"(?m)^\d+\n".to_string(), "[SIZE]\n".to_string()));
        // Filter human-readable sizes (e.g., "384.2 KiB")
        self.filters.push((
            r"(?m)^\d+(\.\d+)? [KMGT]i?B\n".to_string(),
            "[SIZE]\n".to_string(),
        ));
        self
    }
}

#[doc(hidden)] // Macro and test context only, don't use directly.
pub const INSTA_FILTERS: &[(&str, &str)] = &[
    // File sizes
    (r"(\s|\()(\d+\.)?\d+([KM]i)?B", "$1[SIZE]"),
    // Rewrite Windows output to Unix output
    (r"\\([\w\d]|\.\.|\.)", "/$1"),
    (r"prek.exe", "prek"),
    // The exact message is host language dependent
    (
        r"Caused by: .* \(os error 2\)",
        "Caused by: No such file or directory (os error 2)",
    ),
    // Time seconds
    (r"\b(\d+\.)?\d+(ms|s)\b", "[TIME]"),
    // Windows shebang interpreter
    (r"#!/bin/sh", "#!/usr/bin/env bash"),
];

#[allow(unused_macros)]
macro_rules! cmd_snapshot {
    ($spawnable:expr, @$snapshot:literal) => {{
        cmd_snapshot!($crate::common::INSTA_FILTERS.iter().copied().collect::<Vec<_>>(), $spawnable, @$snapshot)
    }};
    ($filters:expr, $spawnable:expr, @$snapshot:literal) => {{
        let mut settings = insta::Settings::clone_current();
        for (matcher, replacement) in $filters {
            settings.add_filter(matcher, replacement);
        }
        let _guard = settings.bind_to_scope();
        insta_cmd::assert_cmd_snapshot!($spawnable, @$snapshot);
    }};
}

#[allow(unused_imports)]
pub(crate) use cmd_snapshot;

#[allow(clippy::disallowed_methods)]
pub(crate) fn remove_bin_from_path(bin: &str, path: Option<OsString>) -> anyhow::Result<OsString> {
    let path = path.unwrap_or(std::env::var_os("PATH").expect("PATH not set"));
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
