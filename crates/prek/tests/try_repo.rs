mod common;

use anyhow::Result;
use assert_cmd::prelude::*;
use assert_fs::fixture::ChildPath;
use assert_fs::prelude::*;
use prek_consts::PRE_COMMIT_HOOKS_YAML;

use crate::common::{TestEnv, cmd_snapshot};

fn with_try_repo_filters(context: TestEnv) -> TestEnv {
    context.with_filters([(r"[a-f0-9]{40}", "[COMMIT_SHA]"), ("'", "\"")])
}

fn create_hook_repo(context: &TestEnv, repo_name: &str) -> String {
    context
        .create_hook_repo(
            repo_name,
            indoc::indoc! {r#"
        - id: test-hook
          name: Test Hook
          entry: echo
          language: system
          files: "\\.txt$"
        - id: another-hook
          name: Another Hook
          entry: python3 -c "print('hello')"
          language: python
    "#},
        )
        // Make the repository an installable Python package.
        .with_file(
            "setup.py",
            "from setuptools import setup; setup(name='dummy-pkg', version='0.0.1')",
        )
        .build()
}

// Helper for a repo with a hook that is designed to fail
fn create_failing_hook_repo(context: &TestEnv, repo_name: &str) -> String {
    context
        .create_hook_repo(
            repo_name,
            indoc::indoc! {r#"
        - id: failing-hook
          name: Always Fail
          entry: "false"
          language: system
        "#},
        )
        .build()
}

#[test]
fn try_repo_basic() {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();

    let repo_path = create_hook_repo(&context, "try-repo-basic");

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
}

#[test]
fn try_repo_failing_hook() {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();

    let repo_path = create_failing_hook_repo(&context, "try-repo-failing");

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
}

#[test]
fn try_repo_specific_hook() {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();

    let repo_path = create_hook_repo(&context, "try-repo-specific-hook");

    context.git().add(".");

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
}

#[test]
fn try_repo_specific_rev() -> Result<()> {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();

    let repo_path = create_hook_repo(&context, "try-repo-specific-rev");
    let git = context.git_at(&repo_path);
    let initial_rev = git.rev_parse("HEAD")?;

    // Make a new commit
    ChildPath::new(&repo_path)
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
        - id: new-hook
          name: New Hook
          entry: echo new
          language: system
        "})?;
    git.add(".").commit("second");

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
    let context = TestEnv::new().with_file("test.txt", "test").init_git();

    let repo_path = create_hook_repo(&context, "try-repo-uncommitted");

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
    context.git_at(&repo_path).add("new-file.txt");

    context.git().add(".");

    let context = with_try_repo_filters(context);

    cmd_snapshot!(context, context.try_repo().arg(&repo_path), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[HOME]/test-repos/try-repo-uncommitted"
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

/// A dirty hook repository should reuse its environment until its contents change.
#[test]
fn try_repo_reuses_environment_for_identical_edits() -> Result<()> {
    let context = TestEnv::new().init_git();
    let repo = context
        .create_hook_repo(
            "hook",
            indoc::indoc! {r#"
        - id: probe
          name: Probe
          language: python
          entry: python -c "import pathlib, sys; pathlib.Path('used-env').write_text(sys.prefix)"
          always_run: true
          pass_filenames: false
    "#},
        )
        .with_file(
            "setup.py",
            "from setuptools import setup; setup(name='probe', version='1')",
        )
        .with_file("version.txt", "one")
        .build();
    let version = ChildPath::new(&repo).child("version.txt");
    let observed = context.work_dir().child("used-env");
    let run = |date: &str| -> Result<String> {
        context
            .try_repo()
            .arg(&repo)
            .arg("-a")
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .assert()
            .success();
        Ok(fs_err::read_to_string(observed.path())?)
    };

    // Advancing Git's clock must not invalidate the environment for the same edit.
    version.write_str("two")?;
    let first = run("2020-01-01T00:00:00Z")?;
    assert_eq!(first, run("2021-01-01T00:00:00Z")?);

    // A different tracked edit needs its own installed environment.
    version.write_str("three")?;
    assert_ne!(first, run("2021-01-01T00:00:00Z")?);
    Ok(())
}

/// A repeated trial can run from its installed snapshot with the submodule offline.
#[test]
fn try_repo_reuses_dirty_snapshot_with_offline_submodule() -> Result<()> {
    let context = TestEnv::new().init_git();
    let upstream = context.home_dir().child("upstream");
    upstream.child("payload").write_str("submodule data")?;
    context
        .git_at(&upstream)
        .init()
        .add(".")
        .commit("Add payload");

    let repo = context
        .create_hook_repo(
            "hook",
            indoc::indoc! {r#"
        - id: read-submodule
          name: Read submodule
          entry: git -C "{hook_repo}/sub" cat-file -e HEAD:payload
          language: system
          always_run: true
          pass_filenames: false
    "#},
        )
        .with_file("tracked", "one")
        .build();
    context
        .git_at(&repo)
        .command()
        .args(["-c", "protocol.file.allow=always", "submodule", "add"])
        .arg(upstream.path())
        .arg("sub")
        .assert()
        .success();
    context.git_at(&repo).commit("Add submodule");
    ChildPath::new(&repo).child("tracked").write_str("two")?;

    let run = || {
        context
            .try_repo()
            .arg(&repo)
            .arg("-a")
            .env("GIT_ALLOW_PROTOCOL", "file")
            .assert()
            .success();
    };
    run();
    fs_err::rename(&upstream, context.home_dir().child("offline"))?;
    run();
    Ok(())
}

/// An expression such as origin/topic is resolved by Git after fetching. A second
/// invocation must see a moved branch even with an already populated trial cache.
#[test]
fn try_repo_follows_moving_remote_expression() -> Result<()> {
    let context = TestEnv::new().init_git();
    let repo = context
        .create_hook_repo(
            "moving",
            indoc::indoc! {r"
        - id: first
          name: First
          language: system
          entry: echo
          always_run: true
    "},
        )
        .build();
    let source = url::Url::from_directory_path(&repo)
        .map_err(|()| anyhow::anyhow!("invalid fixture URL"))?;
    let git = context.git_at(&repo);
    git.branch("topic").checkout("topic");
    context
        .try_repo()
        .arg(source.as_str())
        .args(["--rev", "origin/topic", "first"])
        .assert()
        .success();

    ChildPath::new(&repo)
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(
            "- id: second\n  name: Second\n  language: system\n  entry: echo\n  always_run: true\n",
        )?;
    git.add(".").commit("Move topic");
    let context = context.with_filter(r#"file:[^\n"]*/test-repos/moving/"#, "[SOURCE]");
    cmd_snapshot!(context, context.try_repo().arg(source.as_str()).args(["--rev", "origin/topic", "second"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[SOURCE]"
    rev = "origin/topic"
    hooks = [
      { id = "second" },
    ]

    Second...................................................................Passed

    ----- stderr -----
    "#);
    Ok(())
}

#[test]
fn try_repo_relative_path() {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();

    let _repo_path = create_hook_repo(&context, "try-repo-relative");
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
}

#[test]
fn try_repo_dot_path() -> Result<()> {
    let context = TestEnv::new();
    let repo_path = create_hook_repo(&context, "try-repo-dot");

    ChildPath::new(&repo_path)
        .child("test.txt")
        .write_str("test")?;
    context.git_at(&repo_path).add(".").commit("Add test file");

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
fn try_repo_builtin_hook() {
    let context = TestEnv::new().with_file("test.txt", "test\n").init_git();

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
}

#[test]
fn try_repo_meta_hook() {
    let context = TestEnv::new().with_file("test.txt", "test\n").init_git();

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
}
