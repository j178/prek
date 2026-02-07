mod common;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::ChildPath;
use assert_fs::prelude::*;
use std::path::PathBuf;

use crate::common::{TestContext, cmd_snapshot, git_cmd};
use prek_consts::PRE_COMMIT_HOOKS_YAML;

/// Initialize a git repository at the given path with a commit.
fn init_git_repo(repo_dir: &ChildPath, manifest: &str, include_setup_py: bool) -> Result<()> {
    repo_dir.create_dir_all()?;
    git_cmd(repo_dir).arg("init").assert().success();

    repo_dir.child(PRE_COMMIT_HOOKS_YAML).write_str(manifest)?;

    if include_setup_py {
        repo_dir
            .child("setup.py")
            .write_str("from setuptools import setup; setup(name='dummy-pkg', version='0.0.1')")?;
    }

    git_cmd(repo_dir).arg("add").arg(".").assert().success();
    git_cmd(repo_dir)
        .arg("commit")
        .arg("-m")
        .arg("Initial commit")
        .assert()
        .success();

    Ok(())
}

/// Create a standard hook repository with test-hook and another-hook.
fn create_hook_repo(context: &TestContext, repo_name: &str) -> Result<PathBuf> {
    let repo_dir = context.home_dir().child(format!("test-repos/{repo_name}"));
    init_git_repo(
        &repo_dir,
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
        true, // include setup.py for python hook
    )?;
    Ok(repo_dir.to_path_buf())
}

fn create_failing_hook_repo(context: &TestContext, repo_name: &str) -> Result<PathBuf> {
    let repo_dir = context.home_dir().child(format!("test-repos/{repo_name}"));
    init_git_repo(
        &repo_dir,
        indoc::indoc! {r#"
            - id: failing-hook
              name: Always Fail
              entry: "false"
              language: system
        "#},
        false,
    )?;
    Ok(repo_dir.to_path_buf())
}

fn default_filters(context: &TestContext) -> Vec<(&str, &str)> {
    let mut filters = context.filters();
    filters.push((r"[a-f0-9]{40}", "[COMMIT_SHA]"));
    filters
}

fn setup_basic_context() -> Result<TestContext> {
    let context = TestContext::new();
    context.init_project();
    context.work_dir().child("test.txt").write_str("hello\n")?;
    context.git_add(".");
    Ok(context)
}

#[test]
fn try_repo_basic() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let repo_path = create_hook_repo(&context, "try-repo-basic")?;
    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path).arg("--skip").arg("another-hook"), @r#"
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
    let context = TestContext::new();
    context.init_project();
    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let repo_path = create_failing_hook_repo(&context, "try-repo-failing")?;
    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path), @r#"
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
    let context = TestContext::new();
    context.init_project();
    let repo_path = create_hook_repo(&context, "try-repo-specific-hook")?;

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path).arg("another-hook"), @r#"
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
    let context = TestContext::new();
    context.init_project();
    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let repo_path = create_hook_repo(&context, "try-repo-specific-rev")?;

    let initial_rev = git_cmd(&repo_path)
        .arg("rev-parse")
        .arg("HEAD")
        .output()?
        .stdout;
    let initial_rev = String::from_utf8_lossy(&initial_rev).trim().to_string();

    // Make a new commit with different hooks
    ChildPath::new(&repo_path)
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
            - id: new-hook
              name: New Hook
              entry: echo new
              language: system
        "})?;
    git_cmd(&repo_path).arg("add").arg(".").assert().success();
    git_cmd(&repo_path)
        .arg("commit")
        .arg("-m")
        .arg("second")
        .assert()
        .success();

    let mut filters = default_filters(&context);
    filters.push((initial_rev.as_str(), "[COMMIT_SHA]"));

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path).arg("--ref").arg(&initial_rev), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[HOME]/test-repos/try-repo-specific-rev"
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
    let context = TestContext::new();
    context.init_project();

    let repo_path = create_hook_repo(&context, "try-repo-uncommitted")?;

    // Make uncommitted changes to the hook repo
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
    git_cmd(&repo_path)
        .arg("add")
        .arg("new-file.txt")
        .assert()
        .success();

    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let mut filters = default_filters(&context);
    filters.push((r"try-repo-[^/\\]+", "[REPO]"));

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path), @r#"
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
    warning: Creating temporary repo with uncommitted changes...
    "#);

    Ok(())
}

#[test]
fn try_repo_relative_path() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.work_dir().child("test.txt").write_str("test")?;
    context.git_add(".");

    let _repo_path = create_hook_repo(&context, "try-repo-relative")?;
    let relative_path = "../home/test-repos/try-repo-relative";

    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg(relative_path), @r#"
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
fn try_repo_git_no_matching_hooks() -> Result<()> {
    let context = setup_basic_context()?;
    let repo_path = create_hook_repo(&context, "try-repo-no-matching")?;
    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path).arg("nonexistent-hook").arg("-a"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: No hooks matched the specified selectors for repo `[HOME]/test-repos/try-repo-no-matching`
    ");

    Ok(())
}

#[test]
fn try_repo_config_warning() -> Result<()> {
    let context = setup_basic_context()?;
    let repo_path = create_hook_repo(&context, "try-repo-config-warning")?;

    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg(&repo_path).arg("--config").arg("other.yaml").arg("-a"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "[HOME]/test-repos/try-repo-config-warning"
    rev = "[COMMIT_SHA]"
    hooks = [
      { id = "test-hook" },
      { id = "another-hook" },
    ]

    Test Hook................................................................Passed
    Another Hook.............................................................Passed

    ----- stderr -----
    warning: `--config` option is ignored when using `try-repo`
    "#);

    Ok(())
}

#[test]
fn try_repo_builtin() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a file with trailing whitespace
    context
        .work_dir()
        .child("test.txt")
        .write_str("hello world   \n")?;
    context.git_add(".");

    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg("builtin").arg("trailing-whitespace").arg("-a"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "builtin"
    hooks = [
      { id = "trailing-whitespace" },
    ]

    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing test.txt

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_builtin_multiple_hooks() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child("test.json")
        .write_str("{\"valid\": true}")?;
    context
        .work_dir()
        .child("test.txt")
        .write_str("hello world\n")?;
    context.git_add(".");

    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg("builtin").arg("check-json").arg("trailing-whitespace").arg("-a"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "builtin"
    hooks = [
      { id = "check-json" },
      { id = "trailing-whitespace" },
    ]

    check json...............................................................Passed
    trim trailing whitespace.................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_builtin_all_hooks() -> Result<()> {
    let context = setup_basic_context()?;

    let output = context.try_repo().arg("builtin").arg("-a").output()?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    // no-commit-to-branch fails on the default branch, so we don't assert success
    assert!(stdout.contains("Using generated `prek.toml`:"));
    assert!(stdout.contains(r#"repo = "builtin""#));
    assert!(stdout.contains(r#"{ id = "check-added-large-files" }"#));
    assert!(stdout.contains(r#"{ id = "trailing-whitespace" }"#));

    // Verify builtin hooks are included (exact count is validated by unit tests)
    let hook_count = stdout.matches("{ id =").count();
    assert!(
        hook_count >= 10,
        "Expected at least 10 builtin hooks, found {hook_count}"
    );

    Ok(())
}

#[test]
fn try_repo_builtin_skip() -> Result<()> {
    let context = setup_basic_context()?;
    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg("builtin").arg("check-json").arg("check-merge-conflict").arg("--skip").arg("check-json").arg("-a"), @r#"
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
fn try_repo_meta_identity() -> Result<()> {
    let context = setup_basic_context()?;
    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg("meta").arg("identity").arg("-a"), @r#"
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

#[test]
fn try_repo_meta_all_hooks() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a prek config file for meta hooks that check config
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: trailing-whitespace
    "});

    context.work_dir().child("test.txt").write_str("hello\n")?;
    context.git_add(".");

    let output = context.try_repo().arg("meta").arg("-a").output()?;

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Using generated `prek.toml`:"));
    assert!(stdout.contains(r#"repo = "meta""#));

    // Verify meta hooks are included (exact count is validated by unit tests)
    let hook_count = stdout.matches("{ id =").count();
    assert!(
        hook_count >= 3,
        "Expected at least 3 meta hooks, found {hook_count}"
    );

    Ok(())
}

#[test]
fn try_repo_meta_skip() -> Result<()> {
    let context = setup_basic_context()?;
    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg("meta").arg("identity").arg("check-hooks-apply").arg("--skip").arg("check-hooks-apply").arg("-a"), @r#"
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

#[test]
fn try_repo_meta_check_hooks_apply() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a config with a hook that applies to .rs files (which we don't have)
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust-only
                name: Rust Only
                entry: echo
                language: system
                files: '\\.rs$'
    "});

    context.work_dir().child("test.txt").write_str("hello\n")?;
    context.git_add(".");

    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg("meta").arg("check-hooks-apply").arg("-a"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "meta"
    hooks = [
      { id = "check-hooks-apply" },
    ]

    Check hooks apply........................................................Failed
    - hook id: check-hooks-apply
    - exit code: 1

      rust-only does not apply to this repository

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_meta_check_useless_excludes() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a config with an exclude pattern that doesn't match anything
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: test-hook
                name: Test Hook
                entry: echo
                language: system
                exclude: '\\.nonexistent$'
    "});

    context.work_dir().child("test.txt").write_str("hello\n")?;
    context.git_add(".");

    let filters = default_filters(&context);

    cmd_snapshot!(filters, context.try_repo().arg("meta").arg("check-useless-excludes").arg("-a"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Using generated `prek.toml`:
    [[repos]]
    repo = "meta"
    hooks = [
      { id = "check-useless-excludes" },
    ]

    Check useless excludes...................................................Failed
    - hook id: check-useless-excludes
    - exit code: 1

      The exclude pattern `regex: \/.nonexistent$` for `test-hook` does not match any files

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn try_repo_special_case_insensitive() -> Result<()> {
    let context = setup_basic_context()?;

    for (repo, hook, casings) in [
        (
            "builtin",
            "check-merge-conflict",
            &["BUILTIN", "Builtin", "BuiltIn", "bUILTIN"] as &[&str],
        ),
        ("meta", "identity", &["META", "Meta", "mETA", "MeTa"]),
    ] {
        for casing in casings {
            let output = context
                .try_repo()
                .arg(casing)
                .arg(hook)
                .arg("-a")
                .output()?;
            assert!(
                output.status.success(),
                "{casing} should be recognized as {repo} repo"
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(
                stdout.contains(&format!(r#"repo = "{repo}""#)),
                "{casing} should produce repo = \"{repo}\" in config"
            );
        }
    }

    Ok(())
}

#[test]
fn try_repo_special_ref_warning() -> Result<()> {
    let context = setup_basic_context()?;

    for (repo, hook) in [("builtin", "check-merge-conflict"), ("meta", "identity")] {
        let output = context
            .try_repo()
            .arg(repo)
            .arg("--ref")
            .arg("v1.0.0")
            .arg(hook)
            .arg("-a")
            .output()?;
        assert!(
            output.status.success(),
            "try-repo {repo} with --ref should still succeed"
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&format!(r#"repo = "{repo}""#)),
            "{repo} should appear in generated config"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!("`--ref` option is ignored for `{repo}` repo")),
            "{repo} should warn about --ref being ignored"
        );
    }

    Ok(())
}

#[test]
fn try_repo_special_invalid_hook() -> Result<()> {
    let context = setup_basic_context()?;

    for repo in ["builtin", "meta"] {
        let output = context
            .try_repo()
            .arg(repo)
            .arg("nonexistent-hook")
            .arg("-a")
            .output()?;
        assert!(
            !output.status.success(),
            "try-repo {repo} with invalid hook should fail"
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            stderr.contains(&format!(
                "No hooks matched the specified selectors for repo `{repo}`"
            )),
            "{repo} should report no matching hooks"
        );
    }

    Ok(())
}
