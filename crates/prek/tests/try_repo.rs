mod common;
use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;
use std::path::PathBuf;
use std::process::Command;

use crate::common::{TestContext, cmd_snapshot};
use assert_fs::fixture::ChildPath;
use prek_consts::PRE_COMMIT_HOOKS_YAML;

fn create_hook_repo(context: &TestContext, repo_name: &str) -> Result<PathBuf> {
    let repo_dir = context.home_dir().child(format!("test-repos/{repo_name}"));
    repo_dir.create_dir_all()?;

    Command::new("git")
        .arg("init")
        .current_dir(&repo_dir)
        .assert()
        .success();

    // Configure the author specifically for this hook repository
    Command::new("git")
        .arg("config")
        .arg("user.name")
        .arg("Prek Test")
        .current_dir(&repo_dir)
        .assert()
        .success();
    Command::new("git")
        .arg("config")
        .arg("user.email")
        .arg("test@prek.dev")
        .current_dir(&repo_dir)
        .assert()
        .success();
    // Disable autocrlf for test consistency
    Command::new("git")
        .arg("config")
        .arg("core.autocrlf")
        .arg("false")
        .current_dir(&repo_dir)
        .assert()
        .success();

    repo_dir
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r#"
        - id: test-hook
          name: Test Hook
          entry: echo
          language: system
          files: "\\.txt$"
        - id: another-hook
          name: Another Hook
          entry: python3 -c "print('hello')"
          language: python
    "#})?;

    // Add a dummy setup.py to make it an installable Python package
    repo_dir
        .child("setup.py")
        .write_str("from setuptools import setup; setup(name='dummy-pkg', version='0.0.1')")?;

    Command::new("git")
        .arg("add")
        .arg(".")
        .current_dir(&repo_dir)
        .assert()
        .success();

    Command::new("git")
        .arg("commit")
        .arg("-m")
        .arg("Initial commit")
        .current_dir(&repo_dir)
        .assert()
        .success();

    Ok(repo_dir.to_path_buf())
}

// Helper for a repo with a hook that is designed to fail
fn create_failing_hook_repo(context: &TestContext, repo_name: &str) -> Result<PathBuf> {
    let repo_dir = context.home_dir().child(format!("test-repos/{repo_name}"));
    repo_dir.create_dir_all()?;

    Command::new("git")
        .arg("init")
        .current_dir(&repo_dir)
        .assert()
        .success();
    Command::new("git")
        .arg("config")
        .arg("user.name")
        .arg("Prek Test")
        .current_dir(&repo_dir)
        .assert()
        .success();
    Command::new("git")
        .arg("config")
        .arg("user.email")
        .arg("test@prek.dev")
        .current_dir(&repo_dir)
        .assert()
        .success();
    // Disable autocrlf for test consistency
    Command::new("git")
        .arg("config")
        .arg("core.autocrlf")
        .arg("false")
        .current_dir(&repo_dir)
        .assert()
        .success();

    repo_dir
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r#"
        - id: failing-hook
          name: Always Fail
          entry: "false"
          language: system
        "#})?;

    Command::new("git")
        .arg("add")
        .arg(".")
        .current_dir(&repo_dir)
        .assert()
        .success();

    Command::new("git")
        .arg("commit")
        .arg("-m")
        .arg("Initial commit")
        .current_dir(&repo_dir)
        .assert()
        .success();

    Ok(repo_dir.to_path_buf())
}

#[test]
fn try_repo_basic() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let repo_path = create_hook_repo(&context, "try-repo-basic")?;

    let mut filters = context.filters();
    filters.extend([(r"[a-f0-9]{40}", "[COMMIT_SHA]")]);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path).arg("--skip").arg("another-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Using config:
    repos:
      - repo: [HOME]/test-repos/try-repo-basic
        rev: [COMMIT_SHA]
        hooks:
          - id: test-hook
    Test Hook................................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn try_repo_failing_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let repo_path = create_failing_hook_repo(&context, "try-repo-failing")?;

    let mut filters = context.filters();
    filters.extend([(r"[a-f0-9]{40}", "[COMMIT_SHA]")]);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Using config:
    repos:
      - repo: [HOME]/test-repos/try-repo-failing
        rev: [COMMIT_SHA]
        hooks:
          - id: failing-hook
    Always Fail..............................................................Failed
    - hook id: failing-hook
    - exit code: 1

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn try_repo_specific_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    let repo_path = create_hook_repo(&context, "try-repo-specific-hook")?;

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let mut filters = context.filters();
    filters.extend([(r"[a-f0-9]{40}", "[COMMIT_SHA]")]);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path).arg("another-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Using config:
    repos:
      - repo: [HOME]/test-repos/try-repo-specific-hook
        rev: [COMMIT_SHA]
        hooks:
          - id: another-hook
    Another Hook.............................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn try_repo_specific_rev() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let repo_path = create_hook_repo(&context, "try-repo-specific-rev")?;

    let initial_rev = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(&repo_path)
        .output()?
        .stdout;
    let initial_rev = String::from_utf8_lossy(&initial_rev).trim().to_string();

    // Make a new commit
    ChildPath::new(&repo_path)
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
        - id: new-hook
          name: New Hook
          entry: echo new
          language: system
        "})?;
    Command::new("git")
        .arg("add")
        .arg(".")
        .current_dir(&repo_path)
        .assert()
        .success();
    Command::new("git")
        .arg("commit")
        .arg("-m")
        .arg("second")
        .current_dir(&repo_path)
        .assert()
        .success();

    let mut filters = context.filters();
    filters.extend([
        (r"[a-f0-9]{40}", "[COMMIT_SHA]"),
        (&initial_rev, "[COMMIT_SHA]"),
    ]);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path)
        .arg("--ref")
        .arg(&initial_rev), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Using config:
    repos:
      - repo: [HOME]/test-repos/try-repo-specific-rev
        rev: [COMMIT_SHA]
        hooks:
          - id: test-hook
          - id: another-hook
    Test Hook................................................................Passed
    Another Hook.............................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn try_repo_uncommitted_changes() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    let repo_path = create_hook_repo(&context, "try-repo-uncommitted")?;

    // Make uncommitted changes
    ChildPath::new(&repo_path)
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
        - id: uncommitted-hook
          name: Uncommitted Hook
          entry: echo uncommitted
          language: system
        "})?;
    ChildPath::new(&repo_path)
        .child("new-file.txt")
        .write_str("new")?;
    Command::new("git")
        .arg("add")
        .arg("new-file.txt")
        .current_dir(&repo_path)
        .assert()
        .success();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let mut filters = context.filters();
    filters.extend([
        (r"try-repo-[^/\\]+", "[REPO]"),
        (r"[a-f0-9]{40}", "[COMMIT_SHA]"),
    ]);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Using config:
    repos:
      - repo: [HOME]/scratch/[REPO]/shadow-repo
        rev: [COMMIT_SHA]
        hooks:
          - id: uncommitted-hook
    Uncommitted Hook.........................................................Passed

    ----- stderr -----
    warning: Creating temporary repo with uncommitted changes...
    ");

    Ok(())
}

#[test]
fn try_repo_relative_path() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let _repo_path = create_hook_repo(&context, "try-repo-relative")?;
    let relative_path = "../home/test-repos/try-repo-relative".to_string();

    let mut filters = context.filters();
    filters.extend([(r"[a-f0-9]{40}", "[COMMIT_SHA]")]);

    cmd_snapshot!(filters, context.try_repo().arg(&relative_path), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Using config:
    repos:
      - repo: ../home/test-repos/try-repo-relative
        rev: [COMMIT_SHA]
        hooks:
          - id: test-hook
          - id: another-hook
    Test Hook................................................................Passed
    Another Hook.............................................................Passed

    ----- stderr -----
    ");

    Ok(())
}
