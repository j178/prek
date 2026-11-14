//! Integration tests for detecting file modifications made by hooks.

use std::time::{Duration, SystemTime};

use anyhow::Result;
use assert_fs::prelude::*;

use crate::common::{TestEnv, cmd_snapshot};

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
#[cfg(unix)]
fn identical_rewrite_with_stat_change_is_not_modified() {
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

    for auto_refresh in ["true", "false"] {
        context
            .git()
            .run(["config", "diff.autoRefreshIndex", auto_refresh]);

        insta::allow_duplicates! {
            cmd_snapshot!(context, context.run(), @r#"
            success: true
            exit_code: 0
            ----- stdout -----
            rewrite-identical........................................................Passed

            ----- stderr -----
            "#);
        }
    }
}

#[test]
fn modifying_hook_is_reported() {
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

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    modify...................................................................Failed
    - hook id: modify
    - files were modified by this hook

    ----- stderr -----
    "#);
}

/// Output files, executable reporters, and command overrides must retain
/// modification detection, even when a known remote checker exits successfully.
#[test]
fn remote_report_writer_is_reported() {
    let shell = if cfg!(windows) { "cmd" } else { "sh" };
    let shell_option = format!("shell: {shell}");
    for (url, id, args, overrides, manifest_options) in [
        (
            "https://github.com/hadolint/hadolint",
            "hadolint",
            "--output, report.json",
            "",
            "",
        ),
        (
            "https://github.com/pre-commit/mirrors-jshint",
            "jshint",
            "--reporter, reporter.py",
            "",
            "",
        ),
        (
            "https://github.com/pre-commit/pre-commit-hooks",
            "check-ast",
            "--output, report.json",
            "entry: python3 check.py",
            "",
        ),
        (
            "https://github.com/pre-commit/pre-commit-hooks",
            "check-ast",
            "--output, report.json",
            "language: system",
            "",
        ),
        (
            "https://github.com/pre-commit/pre-commit-hooks",
            "check-ast",
            "--output, report.json",
            shell_option.as_str(),
            "",
        ),
        (
            "https://github.com/pre-commit/pre-commit-hooks",
            "check-ast",
            "--output, report.json",
            "",
            shell_option.as_str(),
        ),
    ] {
        let entry = if overrides == shell_option || manifest_options == shell_option {
            if cfg!(windows) {
                "@python3 check.py %*"
            } else {
                "python3 check.py \"$@\""
            }
        } else {
            "python3 check.py"
        };
        let context = TestEnv::new()
            .with_config(format!(
                indoc::indoc! {r"
                repos:
                  - repo: {url}
                    rev: v1.0.0
                    hooks:
                      - id: {id}
                        args: [{args}]
                        {overrides}
            "},
                url = url,
                id = id,
                args = args,
                overrides = overrides,
            ))
            .with_filter(format!("hook id: {id}"), "hook id: [HOOK_ID]")
            .with_file("report.json", "old report\n")
            .with_file(
                "reporter.py",
                "from pathlib import Path\nPath('report.json').write_text('new report\\n')\n",
            )
            .with_file(
                "check.py",
                indoc::indoc! {r#"
                import argparse
                import runpy
                from pathlib import Path

                parser = argparse.ArgumentParser()
                output = parser.add_mutually_exclusive_group(required=True)
                output.add_argument("--output")
                output.add_argument("--reporter")
                args = parser.parse_args()
                if args.reporter:
                    runpy.run_path(args.reporter)
                else:
                    Path(args.output).write_text("new report\n")
            "#},
            )
            .init_git();
        let repo = context
            .create_hook_repo(
                "report-writer",
                format!(
                    indoc::indoc! {r"
                    - id: {id}
                      name: Write report
                      language: system
                      entry: '{entry}'
                      always_run: true
                      pass_filenames: false
                      {manifest_options}
                "},
                    id = id,
                    entry = entry,
                    manifest_options = manifest_options,
                ),
            )
            .build();

        insta::allow_duplicates! {
            cmd_snapshot!(context, context.run()
                .env("GIT_CONFIG_COUNT", "2")
                .env("GIT_CONFIG_KEY_1", format!("url.{repo}.insteadOf"))
                .env("GIT_CONFIG_VALUE_1", url), @"
            success: false
            exit_code: 1
            ----- stdout -----
            Write report.............................................................Failed
            - hook id: [HOOK_ID]
            - files were modified by this hook

            ----- stderr -----
            ");
        }
    }
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
fn later_project_snapshots_diff_left_by_previous_project() {
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

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    × child
      child-modify...........................................................Failed
      - hook id: child-modify
      - files were modified by this hook
    ✓ <workspace>
      root-noop..............................................................Passed

    ----- stderr -----
    "#);
}

/// An unsupported `read_only` option must not hide a local hook's file changes,
/// in either config format or when the worktree is already dirty.
#[test]
fn read_only_option_does_not_disable_diff_detection() {
    for (filename, config) in [
        (
            ".pre-commit-config.yaml",
            indoc::indoc! {r"
                repos:
                  - repo: local
                    hooks:
                      - id: modify
                        name: modify
                        language: system
                        entry: python3 modify.py
                        pass_filenames: false
                        read_only: true
            "},
        ),
        (
            "prek.toml",
            indoc::indoc! {r#"
                [[repos]]
                repo = "local"
                [[repos.hooks]]
                id = "modify"
                name = "modify"
                language = "system"
                entry = "python3 modify.py"
                pass_filenames = false
                read_only = true
            "#},
        ),
    ] {
        let context = TestEnv::new()
            .with_file(filename, config)
            .with_file("file.txt", "original\n")
            .with_file(
                "modify.py",
                "from pathlib import Path\nPath('file.txt').write_text('modified\\n')\n",
            )
            .with_filter(r"\.pre-commit-config\.yaml|prek\.toml", "[CONFIG]")
            .init_git();
        context.write_file("file.txt", "unstaged\n");

        insta::allow_duplicates! {
            cmd_snapshot!(context, context.run().arg("--all-files"), @"
            success: false
            exit_code: 1
            ----- stdout -----
            modify...................................................................Failed
            - hook id: modify
            - files were modified by this hook

            ----- stderr -----
            warning: Ignored unexpected keys in `[CONFIG]`: `repos[0].hooks[0].read_only`
            ");
        }
    }
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
        stderr.matches("diff_worktree").count(),
        2,
        "The later external hook should capture and compare the modified worktree.\n\
         Trace output:\n{stderr}"
    );

    Ok(())
}

#[test]
fn modifying_builtin_invalidates_baseline_for_later_external_hook() -> Result<()> {
    // A read-only hook between a fixer and an external hook must leave the
    // baseline invalidated so the external hook is not blamed for the fixer's edit.
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
                priority: 0
          - repo: local
            hooks:
              - id: read-only
                name: read-only
                language: pygrep
                entry: not-present
                files: \.txt$
                priority: 1
              - id: noop
                name: noop
                language: system
                entry: python3 -c "pass"
                pass_filenames: false
                priority: 2
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
    assert_eq!(stderr.matches("diff_worktree").count(), 0);

    Ok(())
}
