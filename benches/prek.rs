use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

use assert_cmd::cargo::cargo_bin;
use constants::env_vars::EnvVars;

fn main() {
    divan::main();
}

#[divan::bench]
fn run_local_hooks() {
    fixture().run();
}

struct PrekBenchFixture {
    bin: PathBuf,
    repo_dir: PathBuf,
    store_dir: PathBuf,
    home_dir: PathBuf,
}

static FIXTURE: OnceLock<PrekBenchFixture> = OnceLock::new();

fn fixture() -> &'static PrekBenchFixture {
    FIXTURE.get_or_init(PrekBenchFixture::new)
}

impl PrekBenchFixture {
    fn new() -> Self {
        let bin = cargo_bin("prek");
        let repo_dir = temp_dir("prek-bench-worktree");
        let store_dir = temp_dir("prek-bench-store");
        let home_dir = temp_dir("prek-bench-home");

        init_git_repo(&repo_dir);
        write_local_config(&repo_dir);

        Self::git(&repo_dir, ["add", "."]);
        Self::git(&repo_dir, ["commit", "-m", "Initial commit"]);

        let fixture = Self {
            bin,
            repo_dir,
            store_dir,
            home_dir,
        };

        // Prime caches so subsequent benchmark iterations measure steady-state runs.
        fixture.run();

        fixture
    }

    fn run(&self) {
        let status = self.command().status().expect("failed to run prek");
        assert!(status.success(), "prek run failed during benchmark setup");
    }

    fn command(&self) -> Command {
        let mut cmd = Command::new(&self.bin);
        cmd.current_dir(&self.repo_dir);
        cmd.arg("--quiet");
        cmd.arg("--no-progress");
        cmd.arg("run");
        cmd.arg("--all-files");
        cmd.env(EnvVars::PREK_HOME, &self.store_dir);
        cmd.env(EnvVars::PRE_COMMIT_HOME, &self.home_dir);
        cmd.env(EnvVars::PREK_INTERNAL__SORT_FILENAMES, "1");
        cmd.env("CI", "true");
        cmd.stdout(Stdio::null());
        cmd.stderr(Stdio::null());
        cmd
    }

    fn git(repo: &Path, args: impl IntoIterator<Item = &'static str>) {
        let status = Command::new("git")
            .args(args)
            .current_dir(repo)
            .status()
            .expect("failed to run git");
        assert!(status.success(), "git command failed while preparing benchmark repo");
    }
}

fn temp_dir(prefix: &str) -> PathBuf {
    tempfile::Builder::new()
        .prefix(prefix)
        .tempdir()
        .expect("failed to create temp dir")
        .into_path()
}

fn init_git_repo(repo: &Path) {
    let status = Command::new("git")
        .arg("-c")
        .arg("init.defaultBranch=master")
        .arg("init")
        .current_dir(repo)
        .status()
        .expect("failed to initialize git repository");
    assert!(status.success(), "git init failed");

    PrekBenchFixture::git(repo, ["config", "user.name", "Prek Bench"]);
    PrekBenchFixture::git(repo, ["config", "user.email", "bench@prek.dev"]);
    PrekBenchFixture::git(repo, ["config", "core.autocrlf", "false"]);

    std::fs::write(repo.join("README.md"), "# Prek benchmark\n").expect("failed to write README");
}

fn write_local_config(repo: &Path) {
    let config = r#"repos:
  - repo: local
    hooks:
      - id: git-status
        name: Git status snapshot
        entry: git status --short
        language: system
        pass_filenames: false
"#;

    std::fs::write(repo.join(".pre-commit-config.yaml"), config)
        .expect("failed to write pre-commit config");
}
