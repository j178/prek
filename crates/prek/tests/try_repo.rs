mod common;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;
use std::path::PathBuf;

use crate::common::{TestEnv, cmd_snapshot};
use assert_fs::fixture::ChildPath;
use prek_consts::PRE_COMMIT_HOOKS_YAML;

fn with_try_repo_filters(context: TestEnv) -> TestEnv {
    context.with_filters([(r"[a-f0-9]{40}", "[COMMIT_SHA]"), ("'", "\"")])
}

fn create_hook_repo(context: &TestEnv, repo_name: &str) -> Result<PathBuf> {
    let repo = context.create_repo(repo_name);

    repo.path()
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
    repo.path()
        .child("setup.py")
        .write_str("from setuptools import setup; setup(name='dummy-pkg', version='0.0.1')")?;

    repo.git_add_all();
    repo.git_commit("Initial commit");

    Ok(repo.path().to_path_buf())
}

// Helper for a repo with a hook that is designed to fail
fn create_failing_hook_repo(context: &TestEnv, repo_name: &str) -> Result<PathBuf> {
    let repo = context.create_repo(repo_name);

    repo.path()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r#"
        - id: failing-hook
          name: Always Fail
          entry: "false"
          language: system
        "#})?;

    repo.git_add_all();
    repo.git_commit("Initial commit");

    Ok(repo.path().to_path_buf())
}

#[test]
fn try_repo_basic() -> Result<()> {
    let context = TestEnv::new();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add_all();

    let repo_path = create_hook_repo(&context, "try-repo-basic")?;

    let context = with_try_repo_filters(context);

    cmd_snapshot!(context, context.try_repo().arg(&repo_path).arg("--skip").arg("another-hook"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[HOME]/test-repos/try-repo-basic"
    rev = "[COMMIT_SHA]"
    hooks = [
      { id = "test-hook" },
    ]

    Test Hook................................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_failing_hook() -> Result<()> {
    let context = TestEnv::new();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add_all();

    let repo_path = create_failing_hook_repo(&context, "try-repo-failing")?;

    let context = with_try_repo_filters(context);

    cmd_snapshot!(context, context.try_repo().arg(&repo_path), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[HOME]/test-repos/try-repo-failing"
    rev = "[COMMIT_SHA]"
    hooks = [
      { id = "failing-hook" },
    ]

    Always Fail..............................................................Failed
    - hook id: failing-hook
    - exit code: 1

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_specific_hook() -> Result<()> {
    let context = TestEnv::new();

    let repo_path = create_hook_repo(&context, "try-repo-specific-hook")?;

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add_all();

    let context = with_try_repo_filters(context);

    cmd_snapshot!(context, context.try_repo().arg(&repo_path).arg("another-hook"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[HOME]/test-repos/try-repo-specific-hook"
    rev = "[COMMIT_SHA]"
    hooks = [
      { id = "another-hook" },
    ]

    Another Hook.............................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_specific_rev() -> Result<()> {
    let context = TestEnv::new();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add_all();

    let repo_path = create_hook_repo(&context, "try-repo-specific-rev")?;

    let initial_rev = context
        .git_at(&repo_path)
        .arg("rev-parse")
        .arg("HEAD")
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
    context
        .git_at(&repo_path)
        .arg("add")
        .arg(".")
        .assert()
        .success();
    context
        .git_at(&repo_path)
        .arg("commit")
        .arg("-m")
        .arg("second")
        .assert()
        .success();

    let context = with_try_repo_filters(context).with_filter(initial_rev.clone(), "[COMMIT_SHA]");

    cmd_snapshot!(context, context.try_repo().arg("../home/test-repos/try-repo-specific-rev")
        .arg("--ref")
        .arg(&initial_rev), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "../home/test-repos/try-repo-specific-rev"
    rev = "[COMMIT_SHA]"
    hooks = [
      { id = "test-hook" },
      { id = "another-hook" },
    ]

    Test Hook................................................................Passed
    Another Hook.............................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_uncommitted_changes() -> Result<()> {
    let context = TestEnv::new();

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
    context
        .git_at(&repo_path)
        .arg("add")
        .arg("new-file.txt")
        .assert()
        .success();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add_all();

    let context = context.with_filters([
        (r"try-repo-[^/\\]+", "[REPO]"),
        (r"[a-f0-9]{40}", "[COMMIT_SHA]"),
        ("'", "\""),
    ]);

    cmd_snapshot!(context, context.try_repo().arg(&repo_path), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[HOME]/scratch/[REPO]/shadow-repo"
    rev = "[COMMIT_SHA]"
    hooks = [
      { id = "uncommitted-hook" },
    ]

    Uncommitted Hook.........................................................Passed

    ----- stderr -----
    warning: Local repository has uncommitted changes. Creating a temporary copy...
    "#);

    Ok(())
}

#[test]
fn try_repo_relative_path() -> Result<()> {
    let context = TestEnv::new();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add_all();

    let _repo_path = create_hook_repo(&context, "try-repo-relative")?;
    let relative_path = "../home/test-repos/try-repo-relative".to_string();

    let context = context.with_filter(r"[a-f0-9]{40}", "[COMMIT_SHA]");

    cmd_snapshot!(context, context.try_repo().arg(&relative_path), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "../home/test-repos/try-repo-relative"
    rev = "[COMMIT_SHA]"
    hooks = [
      { id = "test-hook" },
      { id = "another-hook" },
    ]

    Test Hook................................................................Passed
    Another Hook.............................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_dot_path() -> Result<()> {
    let context = TestEnv::new_without_git();
    let repo_path = create_hook_repo(&context, "try-repo-dot")?;

    ChildPath::new(&repo_path)
        .child("test.txt")
        .write_str("test")?;
    context
        .git_at(&repo_path)
        .arg("add")
        .arg(".")
        .assert()
        .success();
    context
        .git_at(&repo_path)
        .arg("commit")
        .arg("-m")
        .arg("Add test file")
        .assert()
        .success();

    let context = context.with_filter(r"[a-f0-9]{40}", "[COMMIT_SHA]");

    cmd_snapshot!(context, context.try_repo().current_dir(&repo_path).arg(".").arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "."
    rev = "[COMMIT_SHA]"
    hooks = [
      { id = "test-hook" },
      { id = "another-hook" },
    ]

    Test Hook................................................................Passed
    Another Hook.............................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_builtin_hook() -> Result<()> {
    let context = TestEnv::new();

    context.work_dir().child("test.txt").write_str("test\n")?;
    context.git_add_all();

    cmd_snapshot!(context, context.try_repo().arg("builtin").arg("check-merge-conflict").arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "builtin"
    hooks = [
      { id = "check-merge-conflict" },
    ]

    check for merge conflicts................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_meta_hook() -> Result<()> {
    let context = TestEnv::new();

    context.work_dir().child("test.txt").write_str("test\n")?;
    context.git_add_all();

    cmd_snapshot!(context, context.try_repo().arg("meta").arg("identity").arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "meta"
    hooks = [
      { id = "identity" },
    ]

    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]

      test.txt

    ----- stderr -----
    "#);

    Ok(())
}
