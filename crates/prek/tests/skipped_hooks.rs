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

use crate::common::{TestContext, cmd_snapshot};

mod common;

/// All hooks skip when no staged files match their file patterns.
#[test]
fn all_hooks_skipped_no_matching_files() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});

    cwd.child("readme.txt").write_str("Hello")?;
    cwd.child("data.json").write_str("{}")?;
    cwd.child("config.yaml").write_str("key: value")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    python-check.........................................(no files to check)Skipped
    rust-check...........................................(no files to check)Skipped
    go-check.............................................(no files to check)Skipped

    ----- stderr -----
    "#);

    Ok(())
}

/// `--dry-run` skips hooks without executing them.
#[test]
fn dry_run_skips_all_hooks() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});

    cwd.child("file.txt").write_str("content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("--dry-run"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    formatter...............................................................Dry Run
    linter..................................................................Dry Run

    ----- stderr -----
    "#);

    assert_eq!(context.read("file.txt"), "content");

    Ok(())
}

/// Hooks that match staged files run; others are skipped.
#[test]
fn mixed_skipped_and_executed_hooks() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});

    cwd.child("readme.txt").write_str("Hello")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    txt-check................................................................Passed
    py-check.............................................(no files to check)Skipped
    rs-check.............................................(no files to check)Skipped

    ----- stderr -----
    "#);

    Ok(())
}

/// Skipped hooks across multiple priority groups
///
/// Hooks with different `priority` values form separate priority groups. Each
/// group is processed sequentially. This test verifies:
/// 1. Skip behavior works correctly across group boundaries
/// 2. `git diff` is only called once (initial baseline), not per-group
///
/// Note: This test uses manual output capture instead of `cmd_snapshot!` because
/// we need to count `get_diff` occurrences in trace-level stderr. Trace output
/// contains non-deterministic timestamps and timing data unsuitable for snapshots.
#[test]
fn all_hooks_skipped_multiple_priority_groups() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});

    cwd.child("data.json").write_str("{}")?;
    context.git_add(".");

    // Run with trace logging to verify #1335 fix
    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(output.status.success(), "prek should succeed");

    // Verify all hooks skipped
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("priority-10") && stdout.contains("Skipped"));
    assert!(stdout.contains("priority-20") && stdout.contains("Skipped"));
    assert!(stdout.contains("priority-30") && stdout.contains("Skipped"));

    // Stashed workflow + all hooks skipped => no diff calls.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let get_diff_calls = stderr.matches("get_diff").count();
    assert_eq!(
        get_diff_calls, 0,
        "Expected 0 get_diff calls when all hooks skip in stashed workflow.\n\
         Found {get_diff_calls} get_diff calls.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

/// When stashed, use `has_worktree_changes` first and fall back to `get_diff` after changes.
#[test]
fn uses_has_worktree_changes_when_stashed() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    // Hook that modifies files (triggers modification detection)
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: modifier
                name: modifier
                language: system
                entry: python3 -c "open('file.txt', 'a').write('modified')"
                files: \.txt$
    "#});

    cwd.child("file.txt").write_str("original")?;
    context.git_add(".");

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    // Hook should fail because it modified files
    assert!(
        !output.status.success(),
        "hook should fail due to file modification"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    // First check uses has_worktree_changes; get_diff only after changes are detected.
    let has_worktree_changes_calls = stderr.matches("has_worktree_changes").count();
    let get_diff_calls = stderr.matches("get_diff").count();

    assert!(
        has_worktree_changes_calls >= 1,
        "Expected has_worktree_changes to be called for first modification check.\n\
         Found {has_worktree_changes_calls} has_worktree_changes calls, {get_diff_calls} get_diff calls.\n\
         Trace output:\n{stderr}"
    );

    // One get_diff call after change detection.
    assert_eq!(
        get_diff_calls, 1,
        "Expected 1 get_diff call (to capture state after change detection).\n\
         Found {get_diff_calls} get_diff calls.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

/// With --all-files (no stash), use full diff comparison to detect new changes.
#[test]
fn uses_get_diff_when_all_files() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    // Hook that modifies files
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: modifier
                name: modifier
                language: system
                entry: python3 -c "open('file.txt', 'a').write('modified')"
                files: \.txt$
    "#});

    cwd.child("file.txt").write_str("original")?;
    // Stage and commit so file is tracked, then use --all-files
    context.git_add(".");
    context.configure_git_author();
    context.git_commit("initial");

    let output = context
        .run()
        .arg("--all-files")
        .env("RUST_LOG", "prek::git=trace")
        .output()?;

    // Hook should fail because it modified files
    assert!(
        !output.status.success(),
        "hook should fail due to file modification"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);

    // With --all-files (no stash), should use get_diff for comparison
    let get_diff_calls = stderr.matches("get_diff").count();
    let has_worktree_changes_calls = stderr.matches("has_worktree_changes").count();

    assert!(
        get_diff_calls >= 1,
        "Expected get_diff to be called for --all-files workflow.\n\
         Found {get_diff_calls} get_diff calls, {has_worktree_changes_calls} has_worktree_changes calls.\n\
         Trace output:\n{stderr}"
    );

    // has_worktree_changes should NOT be used in --all-files workflow
    assert_eq!(
        has_worktree_changes_calls, 0,
        "Expected 0 has_worktree_changes calls in --all-files workflow.\n\
         Found {has_worktree_changes_calls} has_worktree_changes calls.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}
