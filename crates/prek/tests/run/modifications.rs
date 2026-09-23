//! Integration tests for detecting file modifications made by hooks.

use std::time::{Duration, SystemTime};

use anyhow::Result;
use assert_fs::prelude::*;

use crate::common::TestEnv;

fn remove_loose_blob(context: &TestEnv, filename: &str) -> Result<()> {
    let output = context
        .git()
        .command()
        .arg("hash-object")
        .arg(filename)
        .output()?;
    assert!(output.status.success(), "git hash-object should succeed");
    let blob = String::from_utf8(output.stdout)?;
    let blob = blob.trim_ascii();
    let object_path = context
        .child(".git")
        .child("objects")
        .child(&blob[..2])
        .child(&blob[2..]);
    fs_err::remove_file(object_path.path())?;
    Ok(())
}

#[test]
fn external_hook_without_changes_uses_quiet_diff_check() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: noop
                name: noop
                language: system
                entry: python3 -c "pass"
                pass_filenames: false
    "#})
        .with_file("file.txt", "original\n")
        .init_git();

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(output.status.success(), "noop hook should pass");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let has_worktree_diff_calls = stderr.matches("has_worktree_diff").count();
    assert_eq!(
        has_worktree_diff_calls, 1,
        "Expected one cheap worktree diff check, found {has_worktree_diff_calls}.\n\
         Trace output:\n{stderr}"
    );
    let diff_worktree_calls = stderr.matches("diff_worktree").count();
    assert_eq!(
        diff_worktree_calls, 0,
        "Expected no full diff_worktree calls when the hook leaves files unchanged, found {diff_worktree_calls}.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
#[cfg(unix)]
fn identical_rewrite_with_stat_change_is_not_modified() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rewrite-identical
                name: rewrite-identical
                language: system
                entry: python3 rewrite.py
                files: \.txt$
    "})
        .with_file(
            "rewrite.py",
            indoc::indoc! {r"
        from pathlib import Path
        import os
        import sys
        import time

        for filename in sys.argv[1:]:
            path = Path(filename)
            path.write_text(path.read_text())
            timestamp = time.time() + 10
            os.utime(path, (timestamp, timestamp))
    "},
        )
        .with_file("file.txt", "original\n")
        .init_git();

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(
        output.status.success(),
        "rewriting identical content should not be treated as a hook modification"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("rewrite-identical") && stdout.contains("Passed"));
    assert!(!stdout.contains("files were modified by this hook"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    let has_worktree_diff_calls = stderr.matches("has_worktree_diff").count();
    assert_eq!(
        has_worktree_diff_calls, 1,
        "Expected one cheap worktree diff check, found {has_worktree_diff_calls}.\n\
         Trace output:\n{stderr}"
    );

    let diff_worktree_calls = stderr.matches("diff_worktree").count();
    assert_eq!(
        diff_worktree_calls, 1,
        "Expected one content diff to filter out stat-only changes, found {diff_worktree_calls}.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn modifying_hook_uses_clean_baseline_diff_detection() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: modify
                name: modify
                language: system
                entry: python3 -c "from pathlib import Path; Path('file.txt').write_text('changed\n')"
                pass_filenames: false
    "#})
        .with_file("file.txt", "original\n").init_git();

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(
        !output.status.success(),
        "prek should fail when hooks modify files"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("files were modified by this hook"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    let has_worktree_diff_calls = stderr.matches("has_worktree_diff").count();
    assert_eq!(
        has_worktree_diff_calls, 1,
        "Expected one cheap worktree diff check, found {has_worktree_diff_calls}.\n\
         Trace output:\n{stderr}"
    );

    let diff_worktree_calls = stderr.matches("diff_worktree").count();
    assert_eq!(
        diff_worktree_calls, 1,
        "Expected one full diff_worktree call after detecting modifications, found {diff_worktree_calls}.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn binary_diff_snapshots_use_full_object_ids() -> Result<()> {
    let context = TestEnv::new();
    let status = context
        .git()
        .command()
        .args(["init", "--object-format=sha1"])
        .status()?;
    assert!(
        status.success(),
        "initializing SHA-1 repository should succeed"
    );

    let context = context
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: write-first
                name: write-first
                language: system
                entry: python3 -c "from pathlib import Path; Path('binary.dat').write_bytes(b'variant-20663\n')"
                pass_filenames: false
                priority: 0
              - id: write-second
                name: write-second
                language: system
                entry: python3 -c "from pathlib import Path; Path('binary.dat').write_bytes(b'variant-30375\n')"
                pass_filenames: false
                priority: 1
    "#})
        .with_file(".gitattributes", "binary.dat -diff\n")
        .with_file("binary.dat", "original\n");

    // The two replacement blobs have distinct SHA-1s whose first seven
    // hexadecimal digits are both `4b8e34c`.

    context.git().add(".");

    let status = context
        .git()
        .command()
        .args(["config", "core.abbrev", "7"])
        .status()?;
    assert!(status.success(), "setting core.abbrev should succeed");

    let output = context.run().output()?;
    assert!(
        !output.status.success(),
        "both hooks should modify the file"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.matches("files were modified by this hook").count(),
        2,
        "both binary rewrites should produce distinct snapshots.\n\
         stdout:\n{stdout}"
    );

    Ok(())
}

#[test]
fn all_files_with_existing_unstaged_changes_uses_snapshot_baseline() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: modify
                name: modify
                language: system
                entry: python3 -c "from pathlib import Path; Path('hook.txt').write_text('changed\n')"
                pass_filenames: false
    "#})
        .with_file("file.txt", "original\n")
        .with_file("hook.txt", "original\n").init_git();

    context.write_file("file.txt", "unstaged\n");

    let output = context
        .run()
        .arg("--all-files")
        .env("RUST_LOG", "prek::git=trace")
        .output()?;

    assert!(
        !output.status.success(),
        "--all-files should still detect hook modifications when the worktree starts dirty"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("files were modified by this hook"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    let has_worktree_diff_calls = stderr.matches("has_worktree_diff").count();
    assert_eq!(
        has_worktree_diff_calls, 0,
        "`--all-files` should not use the clean-baseline diff check.\n\
         Trace output:\n{stderr}"
    );

    let diff_worktree_calls = stderr.matches("diff_worktree").count();
    assert_eq!(
        diff_worktree_calls, 2,
        "Expected a full before/after diff comparison for dirty `--all-files`, found {diff_worktree_calls}.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn all_files_clean_missing_blob_ignores_diff_snapshot_errors() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: noop
                name: noop
                language: system
                entry: python3 -c "pass"
                pass_filenames: false
    "#})
        .with_file("file.txt", "original\n")
        .init_git();

    context.git().commit("init");

    remove_loose_blob(&context, "file.txt")?;

    // Make the index stat data stale while keeping file content unchanged. A
    // full `git diff` now exits non-zero because the blob is missing, but its
    // stdout is still a usable best-effort before/after snapshot.
    fs_err::OpenOptions::new()
        .write(true)
        .open(context.child("file.txt").path())?
        .set_modified(SystemTime::now() + Duration::from_secs(10))?;

    let output = context
        .run()
        .arg("--all-files")
        .env("RUST_LOG", "prek::git=trace")
        .output()?;

    assert!(
        output.status.success(),
        "`--all-files` should not require blob objects when hooks leave a clean tree.\n\
         stdout:\n{}\n\
         stderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let ignored_diff_errors = stderr
        .matches("Continuing with git diff stdout despite non-zero exit status")
        .count();
    assert_eq!(
        ignored_diff_errors, 2,
        "Expected before/after git diff errors to be logged and ignored, found {ignored_diff_errors}.\n\
         Trace output:\n{stderr}"
    );
    assert!(
        !stderr.contains("Command `git diff` exited with an error"),
        "missing blobs should not turn hook modification detection into a fatal git diff error.\n\
         stderr:\n{stderr}"
    );

    Ok(())
}

#[test]
fn later_project_snapshots_diff_left_by_previous_project() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: root-noop
                name: root-noop
                language: system
                entry: python3 -c "pass"
                always_run: true
                pass_filenames: false
    "#})
        .with_file(
            "child/.pre-commit-config.yaml",
            indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: child-modify
                name: child-modify
                language: system
                entry: python3 -c "from pathlib import Path; Path('child.txt').write_text('changed\n')"
                always_run: true
                pass_filenames: false
    "#},
        )
        .with_file("child/child.txt", "original\n").init_git();

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(
        !output.status.success(),
        "prek should fail because the child hook modified files"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("child-modify") && stdout.contains("files were modified by this hook"));
    assert!(
        stdout.contains("root-noop") && stdout.contains("Passed"),
        "root hook should not be blamed for the child project's diff.\n\
         stdout:\n{stdout}"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    let has_worktree_diff_calls = stderr.matches("has_worktree_diff").count();
    assert_eq!(
        has_worktree_diff_calls, 1,
        "Only the first project should use the clean-baseline check.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn read_only_builtin_hook_does_not_run_diff_detection() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-toml
    "})
        .with_file("pyproject.toml", "[project]\nname = \"demo\"\n")
        .init_git();

    let output = context
        .run()
        .arg("--all-files")
        .env("RUST_LOG", "prek::git=trace")
        .output()?;

    assert!(output.status.success(), "prek should succeed");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let diff_worktree_calls = stderr.matches("diff_worktree").count();
    assert_eq!(
        diff_worktree_calls, 0,
        "Expected no diff_worktree calls for read-only builtin hooks, found {diff_worktree_calls}.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn read_only_languages_do_not_run_diff_detection() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: fail
                name: fail
                language: fail
                entry: expected failure
                files: \.txt$
              - id: pygrep
                name: pygrep
                language: pygrep
                entry: not-present
                files: \.txt$
    "})
        .with_file("file.txt", "original\n")
        .init_git();

    let output = context
        .run()
        .arg("--all-files")
        .env("RUST_LOG", "prek::git=trace")
        .output()?;

    assert!(!output.status.success(), "the fail hook should fail");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("fail") && stdout.contains("Failed"));
    assert!(stdout.contains("pygrep") && stdout.contains("Passed"));
    assert!(!stdout.contains("files were modified by this hook"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("has_worktree_diff").count(),
        0,
        "Read-only languages should not require a worktree diff check.\n\
         Trace output:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("diff_worktree").count(),
        0,
        "Read-only languages should not require a full worktree diff.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn same_group_known_modification_skips_diff_detection() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
                priority: 0
          - repo: local
            hooks:
              - id: noop
                name: noop
                language: system
                entry: python3 -c "pass"
                pass_filenames: false
                priority: 0
    "#})
        .with_file("file.txt", "missing newline")
        .init_git();

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(
        !output.status.success(),
        "the builtin should report its modification"
    );
    assert_eq!(context.read("file.txt"), "missing newline\n");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("noop") && line.contains("Passed"))
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("has_worktree_diff").count(),
        0,
        "A known modification should make the same-group quiet diff unnecessary.\n\
         Trace output:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("diff_worktree").count(),
        0,
        "A known modification should make the same-group full diff unnecessary.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn same_group_known_modification_rebaselines_later_external_hook() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
                priority: 0
          - repo: local
            hooks:
              - id: same-group-noop
                name: same-group-noop
                language: system
                entry: python3 -c "pass"
                pass_filenames: false
                priority: 0
              - id: later-noop
                name: later-noop
                language: system
                entry: python3 -c "pass"
                pass_filenames: false
                priority: 1
    "#})
        .with_file("file.txt", "missing newline")
        .init_git();

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(
        !output.status.success(),
        "the builtin should report its modification"
    );
    assert_eq!(context.read("file.txt"), "missing newline\n");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout
            .lines()
            .any(|line| line.contains("later-noop") && line.contains("Passed"))
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("has_worktree_diff").count(),
        0,
        "The known modification should invalidate the clean baseline.\n\
         Trace output:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("diff_worktree").count(),
        2,
        "The later external hook should capture and compare the modified worktree.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn modifying_builtin_invalidates_baseline_for_later_external_hook() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
                priority: 0
          - repo: local
            hooks:
              - id: noop
                name: noop
                language: system
                entry: python3 -c "pass"
                pass_filenames: false
                priority: 1
    "#})
        .with_file("file.txt", "missing newline")
        .init_git();

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(
        !output.status.success(),
        "the builtin should report its modification"
    );
    assert_eq!(context.read("file.txt"), "missing newline\n");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("end-of-file-fixer") && stdout.contains("files were modified by this hook")
    );
    assert!(stdout.contains("noop") && stdout.contains("Passed"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        stderr.matches("has_worktree_diff").count(),
        0,
        "The builtin result and dirty baseline should avoid the clean-worktree check.\n\
         Trace output:\n{stderr}"
    );
    assert_eq!(
        stderr.matches("diff_worktree").count(),
        2,
        "The later external hook should snapshot the builtin's change, then compare against it.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn failed_non_modifying_builtin_skips_diff_detection() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: mixed-line-ending
                args: ['--fix=no']
    "})
        .with_file("mixed.txt", "first\r\nsecond\n")
        .init_git();

    let output = context.run().env("RUST_LOG", "prek::git=trace").output()?;

    assert!(
        !output.status.success(),
        "mixed-line-ending should report the validation failure"
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("mixed.txt: mixed line endings"));
    assert!(!stdout.contains("files were modified by this hook"));

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("has_worktree_diff").count(), 0);
    assert_eq!(stderr.matches("diff_worktree").count(), 0);

    Ok(())
}
