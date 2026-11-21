mod common;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
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

    let context = context.with_filters([
        (r"try-repo-shadow-[0-9a-f]{16}", "[SHADOW]"),
        (r"[a-f0-9]{40}", "[COMMIT_SHA]"),
        ("'", "\""),
    ]);

    cmd_snapshot!(context, context.try_repo().arg(&repo_path), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[HOME]/scratch/[SHADOW]"
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

/// List the entry names of a store directory in sorted order.
fn store_dir_entries(context: &TestEnv, dir: &str) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in fs_err::read_dir(context.home_dir().child(dir).path())? {
        names.push(entry?.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    Ok(names)
}

/// List the hook environments and repository clones in the store.
fn store_entries(context: &TestEnv) -> Result<(Vec<String>, Vec<String>)> {
    Ok((
        store_dir_entries(context, "hooks")?,
        store_dir_entries(context, "repos")?,
    ))
}

/// The generated config and the shadow copy must not outlive the command.
fn assert_no_try_repo_scratch(context: &TestEnv) -> Result<()> {
    let leftovers: Vec<_> = store_dir_entries(context, "scratch")?
        .into_iter()
        .filter(|name| name.starts_with("try-repo"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "unexpected scratch entries: {leftovers:?}"
    );
    Ok(())
}

#[test]
fn try_repo_reuses_installed_environment() -> Result<()> {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();

    let repo_path = create_hook_repo(&context, "try-repo-reuse");

    context
        .try_repo()
        .arg(&repo_path)
        .arg("another-hook")
        .assert()
        .success();
    let (envs, repos) = store_entries(&context)?;
    assert_eq!(envs.len(), 1, "expected one hook env, found {envs:?}");
    assert_eq!(repos.len(), 1, "expected one repo clone, found {repos:?}");

    context
        .try_repo()
        .arg(&repo_path)
        .arg("another-hook")
        .assert()
        .success();
    assert_eq!(store_entries(&context)?, (envs, repos));
    assert_no_try_repo_scratch(&context)?;

    Ok(())
}

#[test]
fn try_repo_uncommitted_changes_reuses_environment() -> Result<()> {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();

    let repo_path = create_hook_repo(&context, "try-repo-uncommitted-reuse");
    let staged = ChildPath::new(&repo_path).child("staged.txt");
    staged.write_str("first")?;
    context.git_at(&repo_path).add("staged.txt");

    context
        .try_repo()
        .arg(&repo_path)
        .arg("another-hook")
        .assert()
        .success();
    let (envs, repos) = store_entries(&context)?;
    assert_eq!(envs.len(), 1, "expected one hook env, found {envs:?}");
    assert_eq!(repos.len(), 1, "expected one repo clone, found {repos:?}");
    assert_no_try_repo_scratch(&context)?;

    // Unchanged uncommitted contents produce the same shadow revision. Git timestamps have
    // one-second resolution, so simulate a later run to show the commit ignores the clock.
    context
        .try_repo()
        .arg(&repo_path)
        .arg("another-hook")
        .env("GIT_AUTHOR_DATE", "2030-01-01T00:00:00+00:00")
        .env("GIT_COMMITTER_DATE", "2030-01-01T00:00:00+00:00")
        .assert()
        .success();
    assert_eq!(store_entries(&context)?, (envs.clone(), repos.clone()));

    // Changed contents produce a new revision, which needs its own clone and environment.
    staged.write_str("second")?;
    context.git_at(&repo_path).add("staged.txt");
    context
        .try_repo()
        .arg(&repo_path)
        .arg("another-hook")
        .assert()
        .success();
    let (new_envs, new_repos) = store_entries(&context)?;
    assert_eq!(
        new_envs.len(),
        2,
        "expected two hook envs, found {new_envs:?}"
    );
    assert_eq!(
        new_repos.len(),
        2,
        "expected two repo clones, found {new_repos:?}"
    );
    assert_no_try_repo_scratch(&context)?;

    Ok(())
}

/// Try `--ref topic`, then move the `topic` branch to a commit with a different manifest.
///
/// Returns a context whose filters hide the source, so the next run's snapshot shows whether it
/// used the moved branch instead of the clone cached for the old commit.
fn move_tried_branch(context: TestEnv, repo_path: &str, source: &str) -> Result<TestEnv> {
    let git = context.git_at(repo_path);
    git.branch("topic");
    context
        .try_repo()
        .arg(source)
        .arg("--ref")
        .arg("topic")
        .assert()
        .success();

    git.checkout("topic");
    ChildPath::new(repo_path)
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
        - id: new-hook
          name: New Hook
          entry: echo new
          language: system
        "})?;
    git.add(".").commit("Move topic");

    Ok(context.with_filters([
        (
            r"(file:/+)?\[HOME\]/test-repos/try-repo-[a-z-]+",
            "[SOURCE]",
        ),
        ("'", "\""),
    ]))
}

#[test]
fn try_repo_local_ref_follows_moved_branch() -> Result<()> {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();
    let repo_path = create_hook_repo(&context, "try-repo-local-moved-branch");
    let context = move_tried_branch(context, &repo_path, &repo_path)?;

    cmd_snapshot!(context, context.try_repo().arg(&repo_path).arg("--ref").arg("topic"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[SOURCE]"
    rev = "topic"
    hooks = [
      { id = "new-hook" },
    ]

    New Hook.................................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[cfg(unix)]
#[test]
fn try_repo_remote_ref_follows_moved_branch() -> Result<()> {
    let context = TestEnv::new().with_file("test.txt", "test").init_git();
    let repo_path = create_hook_repo(&context, "try-repo-remote-moved-branch");
    let url = format!("file://{repo_path}");
    let context = move_tried_branch(context, &repo_path, &url)?;

    cmd_snapshot!(context, context.try_repo().arg(&url).arg("--ref").arg("topic"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[SOURCE]"
    rev = "topic"
    hooks = [
      { id = "new-hook" },
    ]

    New Hook.................................................................Passed

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
