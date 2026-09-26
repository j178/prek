//! Integration tests for hook skip behavior.
//!
//! These tests verify that prek correctly identifies and reports skipped hooks
//! in various scenarios: file pattern mismatches, dry-run mode, and mixed
//! execution across priority groups.
//!
//! Includes regression tests for #1335: when all hooks in a group are skipped,
//! prek should not call `git diff` to check for file modifications.

use anyhow::Result;
use assert_fs::prelude::*;

use crate::common::{TestEnv, cmd_snapshot};

fn hook_env_count(context: &TestEnv) -> Result<usize> {
    let hooks_dir = context.home_dir().child("hooks");
    if !hooks_dir.exists() {
        return Ok(0);
    }
    Ok(hooks_dir.read_dir()?.count())
}

/// All hooks skip when no staged files match their file patterns.
#[test]
fn all_hooks_skipped_no_matching_files() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: python-check
                name: python-check
                language: system
                entry: echo "checking python"
                files: \.py$
              - id: rust-check
                name: rust-check
                language: system
                entry: echo "checking rust"
                files: \.rs$
              - id: go-check
                name: go-check
                language: system
                entry: echo "checking go"
                files: \.go$
    "#})
        .with_file("readme.txt", "Hello")
        .with_file("data.json", "{}")
        .with_file("config.yaml", "key: value")
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    python-check.........................................(no files to check)Skipped
    rust-check...........................................(no files to check)Skipped
    go-check.............................................(no files to check)Skipped

    ----- stderr -----
    "#);
}

/// Installable hooks with no matching files should not create environments.
#[test]
fn skipped_installable_hook_does_not_install_env() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: python-check
                name: python-check
                language: python
                entry: python -c "print('checking python')"
                files: \.py$
    "#})
        .with_file("README.md", "Hello")
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    python-check.........................................(no files to check)Skipped

    ----- stderr -----
    "#);

    assert_eq!(hook_env_count(&context)?, 0);

    Ok(())
}

/// Installable hooks excluded by group selection should not create environments.
#[test]
fn group_excluded_installable_hook_does_not_install_env() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: selected
                name: selected
                language: system
                entry: python3 -c "print('selected')"
                always_run: true
                groups: [ci]
              - id: excluded-python
                name: excluded-python
                language: python
                entry: python -c "print('excluded')"
                always_run: true
                groups: [slow]
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("--group").arg("ci"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    selected.................................................................Passed

    ----- stderr -----
    "#);

    assert_eq!(hook_env_count(&context)?, 0);

    Ok(())
}

/// `always_run` installable hooks still install and run without matching files.
#[test]
fn always_run_installable_hook_installs_without_matching_files() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: always-python
                name: always-python
                language: python
                entry: python -c "print('ran')"
                files: \.py$
                always_run: true
                pass_filenames: false
    "#})
        .with_file("README.md", "Hello")
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    always-python............................................................Passed

    ----- stderr -----
    "#);

    assert_eq!(hook_env_count(&context)?, 1);

    Ok(())
}

/// `--dry-run` skips hooks without executing them.
#[test]
fn dry_run_skips_all_hooks() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: formatter
                name: formatter
                language: system
                entry: python3 -c "import sys; open(sys.argv[1], 'a').write('modified')"
                files: \.txt$
              - id: linter
                name: linter
                language: system
                entry: echo "linting"
                files: \.txt$
    "#})
        .with_file("file.txt", "content")
        .init_git();

    cmd_snapshot!(context, context.run().arg("--dry-run"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    formatter...............................................................Dry Run
    linter..................................................................Dry Run

    ----- stderr -----
    "#);

    assert_eq!(context.read("file.txt"), "content");
}

/// Hooks that match staged files run; others are skipped.
#[test]
fn mixed_skipped_and_executed_hooks() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: txt-check
                name: txt-check
                language: system
                entry: echo "checking txt"
                files: \.txt$
              - id: py-check
                name: py-check
                language: system
                entry: echo "checking py"
                files: \.py$
              - id: rs-check
                name: rs-check
                language: system
                entry: echo "checking rs"
                files: \.rs$
    "#})
        .with_file("readme.txt", "Hello")
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    txt-check................................................................Passed
    py-check.............................................(no files to check)Skipped
    rs-check.............................................(no files to check)Skipped

    ----- stderr -----
    "#);
}

/// Skipped hooks in untouched workspace projects should not install environments.
#[test]
fn skipped_workspace_project_installable_hook_does_not_install_env() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: root-skip
                name: root-skip
                language: system
                entry: echo root
                files: \.root$
    "})
        .with_file(
            "proj-a/.pre-commit-config.yaml",
            indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: proj-a-check
                name: proj-a-check
                language: system
                entry: echo proj-a
                files: \.txt$
    "},
        )
        .with_file(
            "proj-b/.pre-commit-config.yaml",
            indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: proj-b-python
                name: proj-b-python
                language: python
                entry: python -c "print('proj-b')"
                files: \.py$
    "#},
        )
        .with_file("proj-a/README.txt", "Hello")
        .init_git();

    let output = context.run().output()?;
    assert!(output.status.success(), "prek should succeed");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("proj-a-check") && stdout.contains("Passed"));
    assert!(stdout.contains("proj-b-python") && stdout.contains("Skipped"));
    assert_eq!(hook_env_count(&context)?, 0);

    Ok(())
}

#[test]
fn orphan_project_early_match_still_hides_child_files_from_parent_install() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: root-pygrep
                name: root-pygrep
                language: pygrep
                entry: ROOT_SHOULD_NOT_RUN
                files: \.py$
    "})
        .with_file(
            "child/.pre-commit-config.yaml",
            indoc::indoc! {r#"
        orphan: true
        repos:
          - repo: local
            hooks:
              - id: child-python
                name: child-python
                language: python
                entry: python -c "print('child')"
                always_run: true
                pass_filenames: false
    "#},
        )
        .with_file("child/child.py", "print('child')\n")
        .init_git();

    cmd_snapshot!(context, context.run().arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ child
      child-python...........................................................Passed
    ✓ <workspace>
      root-pygrep........................................(no files to check)Skipped

    ----- stderr -----
    "#);
    assert_eq!(hook_env_count(&context)?, 1);

    Ok(())
}

/// Skipped hooks across multiple priority groups
///
/// Hooks with different `priority` values form separate priority groups. Each
/// group is processed sequentially. This test verifies:
/// 1. Skip behavior works correctly across group boundaries
/// 2. `git diff` is not called when every hook is skipped
///
/// Note: This test uses manual output capture instead of `cmd_snapshot!` because
/// we need to count `diff_worktree` occurrences in trace-level stderr. Trace output
/// contains non-deterministic timestamps and timing data unsuitable for snapshots.
#[test]
fn all_hooks_skipped_multiple_priority_groups() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: priority-10
                name: priority-10
                language: system
                entry: echo "priority 10"
                files: \.py$
                priority: 10
              - id: priority-20
                name: priority-20
                language: system
                entry: echo "priority 20"
                files: \.rs$
                priority: 20
              - id: priority-30
                name: priority-30
                language: system
                entry: echo "priority 30"
                files: \.go$
                priority: 30
    "#})
        .with_file("data.json", "{}")
        .init_git();

    // Run with trace logging to verify #1335 fix
    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(output.status.success(), "prek should succeed");

    // Verify all hooks skipped
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("priority-10") && stdout.contains("Skipped"));
    assert!(stdout.contains("priority-20") && stdout.contains("Skipped"));
    assert!(stdout.contains("priority-30") && stdout.contains("Skipped"));

    // Regression test for #1335: skipped hooks do not need modification checks.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let diff_worktree_calls = stderr.matches("diff_worktree").count();
    assert_eq!(
        diff_worktree_calls, 0,
        "Expected no diff_worktree calls when all hooks skip, found {diff_worktree_calls}.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}
