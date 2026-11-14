use crate::common::{TestEnv, cmd_snapshot};

/// Native, fast-path, and annotated external hooks must all reach Git before
/// any can finish, even with a skipped fail-fast hook between them.
#[cfg(unix)]
#[test]
fn read_only_hooks_run_concurrently() -> anyhow::Result<()> {
    use std::net::TcpListener;
    use std::time::Duration;

    use prek_consts::env_vars::{EnvVars, EnvVarsRead};
    use tokio::io::AsyncWriteExt;

    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
            repos:
              - repo: builtin
                hooks:
                  - id: no-commit-to-branch
                    name: First check
                    args: [--branch, protected]
              - repo: local
                hooks:
                  - id: skipped
                    name: Skipped check
                    language: fail
                    entry: should not run
                    always_run: true
                    fail_fast: true
              - repo: https://github.com/pre-commit/pre-commit-hooks
                rev: v1.0.0
                hooks:
                  - id: no-commit-to-branch
                    args: [--branch, protected]
                  - id: check-ast
        "})
        .init_git();
    let repo = context
        .create_hook_repo(
            "checks",
            indoc::indoc! {r"
                - id: no-commit-to-branch
                  name: Second check
                  language: system
                  entry: no-commit-to-branch
                  always_run: true
                  pass_filenames: false
                - id: check-ast
                  name: External check
                  language: system
                  entry: git symbolic-ref HEAD
                  always_run: true
                  pass_filenames: false
            "},
        )
        .build();
    context.write_executable_file(
        ".git/bin/git",
        indoc::indoc! {r"
            #!/usr/bin/env python3
            import os
            import socket
            import subprocess
            import sys

            args = sys.argv[1:]
            result = subprocess.run([os.environ['REAL_GIT'], *args])
            if 'symbolic-ref' in args:
                host, port = os.environ['CHECK_ADDRESS'].split(':')
                with socket.create_connection((host, int(port)), timeout=30) as sock:
                    assert sock.recv(1) == b'x'
            sys.exit(result.returncode)
        "},
    );

    let listener = TcpListener::bind("127.0.0.1:0")?;
    let address = listener.local_addr()?;
    listener.set_nonblocking(true)?;
    let barrier = std::thread::spawn(move || -> anyhow::Result<()> {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(async move {
                let listener = tokio::net::TcpListener::from_std(listener)?;
                tokio::time::timeout(Duration::from_secs(30), async {
                    let mut sockets = Vec::new();
                    for _ in 0..3 {
                        sockets.push(listener.accept().await?.0);
                    }
                    for mut socket in sockets {
                        socket.write_all(b"x").await?;
                    }
                    anyhow::Ok(())
                })
                .await?
            })
    });
    let path = std::env::join_paths(
        std::iter::once(context.child(".git/bin").to_path_buf()).chain(std::env::split_paths(
            &EnvVars.var_os(EnvVars::PATH).unwrap_or_default(),
        )),
    )?;
    let real_git = which::which("git")?;

    cmd_snapshot!(context, context.run()
        .args(["-q", "--skip", "skipped"])
        .env("PATH", path)
        .env("REAL_GIT", real_git)
        .env("CHECK_ADDRESS", address.to_string())
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_1", format!("url.{repo}.insteadOf"))
        .env("GIT_CONFIG_VALUE_1", "https://github.com/pre-commit/pre-commit-hooks")
        .env("PREK_CONCURRENT_HOOKS", "3")
        .env_remove("PREK_NO_CONCURRENCY")
        .env_remove("PREK_NO_FAST_PATH"), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");

    barrier
        .join()
        .map_err(|_| anyhow::anyhow!("Read-only hook barrier panicked"))??;
    Ok(())
}

/// A workspace-read-only external hook skips diff checks but still waits for the
/// previous group's report. One worker makes accidental grouping observable.
#[test]
fn workspace_read_only_hooks_keep_execution_barriers() -> anyhow::Result<()> {
    use crate::common::snapshot;

    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
            repos:
              - repo: builtin
                hooks:
                  - id: no-commit-to-branch
                    name: Earlier check
                    args: [--branch, protected]
              - repo: https://github.com/koalaman/shellcheck-precommit
                rev: v1.0.0
                hooks:
                  - id: shellcheck
        "})
        .with_file("file.txt", "original\n")
        .with_filter(r"(?m)^.*TRACE[^\n]*diff_worktree[^\n]*\n", "git diff\n")
        .with_filter(r"(?m)^.*(?:TRACE|DEBUG) [^\n]*\n", "")
        .init_git();
    let repo = context
        .create_hook_repo(
            "checks",
            indoc::indoc! {r#"
                - id: shellcheck
                  name: Reads earlier report
                  language: system
                  entry: >-
                    python3 -c "from pathlib import Path; assert Path('.git/report').read_text()"
                  always_run: true
                  pass_filenames: false
            "#},
        )
        .build();
    context.write_file("file.txt", "unstaged\n");
    let report = std::fs::File::create(context.child(".git/report"))?;

    cmd_snapshot!(context, context.run()
        .args(["--all-files", "--color=never"])
        .env("PREK_CONCURRENT_HOOKS", "1")
        .env("PREK_NO_FAST_PATH", "1")
        .env("GIT_CONFIG_COUNT", "2")
        .env("GIT_CONFIG_KEY_1", format!("url.{repo}.insteadOf"))
        .env("GIT_CONFIG_VALUE_1", "https://github.com/koalaman/shellcheck-precommit")
        .env("RUST_LOG", "prek::git=trace")
        .stdout(report), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");
    snapshot!(context, context.read(".git/report"), @"
    Earlier check............................................................Passed
    Reads earlier report.....................................................Passed
    ");
    Ok(())
}

/// A builtin writer separates read-only runs and retains single-hook change
/// attribution. One worker makes an accidental merge fail deterministically.
#[test]
fn read_only_hooks_do_not_merge_across_a_fixer() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
            repos:
              - repo: builtin
                hooks:
                  - id: check-json
                    name: Before fixer
                  - id: end-of-file-fixer
                  - id: check-json
                    name: After fixer
        "})
        .with_file("file.json", "{}")
        .init_git();

    cmd_snapshot!(context, context.run()
        .args(["--files", "file.json"])
        .env("PREK_CONCURRENT_HOOKS", "1"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    Before fixer.............................................................Passed
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - description: Ensures that a file is either empty, or ends with one newline
    - exit code: 1
    - files were modified by this hook

      Fixing file.json
    After fixer..............................................................Passed

    ----- stderr -----
    ");
    assert_eq!(context.read("file.json"), "{}\n");
}

/// An explicit priority on either side keeps the earlier report visible to a
/// later read-only hook. Merging the groups would leave that report empty.
#[test]
fn explicit_priority_separates_read_only_hooks() -> anyhow::Result<()> {
    use crate::common::snapshot;

    for (first_priority, second_priority) in [("priority: 0", ""), ("", "priority: 1")] {
        let context = TestEnv::new()
            .with_config(format!(
                indoc::indoc! {r"
                    repos:
                      - repo: builtin
                        hooks:
                          - id: no-commit-to-branch
                            name: Earlier check
                            args: [--branch, protected]
                            {first_priority}
                          - id: check-toml
                            name: Reads earlier report
                            args: [.git/report]
                            pass_filenames: false
                            always_run: true
                            {second_priority}
                "},
                first_priority = first_priority,
                second_priority = second_priority
            ))
            .init_git();
        let report = std::fs::File::create(context.child(".git/report"))?;

        insta::allow_duplicates! {
            cmd_snapshot!(context, context.run()
                .arg("--color=never")
                .stdout(report), @"
            success: false
            exit_code: 1
            ----- stdout -----

            ----- stderr -----
            ");
            snapshot!(context, context.read(".git/report"), @"
            Earlier check............................................................Passed
            Reads earlier report.....................................................Failed
            - hook id: check-toml
            - description: Checks TOML files for parseable syntax
            - exit code: 1

              .git/report: Failed to toml decode (TOML parse error at line 1, column 9
                |
              1 | Earlier check............................................................Passed
                |         ^
              key with no value, expected `=`
              )
            ");
        }
    }
    Ok(())
}

#[test]
fn priorities_respected() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                name: Late Hook
                args: [--branch, protected]
                priority: 10
              - id: no-commit-to-branch
                name: Early Hook
                args: [--branch, protected]
                priority: 0
              - id: no-commit-to-branch
                name: Middle Hook
                args: [--branch, protected]
                priority: 5
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Early Hook...............................................................Passed
    Middle Hook..............................................................Passed
    Late Hook................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn priority_aliases_are_resolved_before_scheduling() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        priorities:
          early: 0
          late: 10
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                name: Late Hook
                args: [--branch, protected]
                priority: late
              - id: no-commit-to-branch
                name: Early Hook
                args: [--branch, protected]
                priority: early
              - id: no-commit-to-branch
                name: Numeric Hook
                args: [--branch, protected]
                priority: 5
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Early Hook...............................................................Passed
    Numeric Hook.............................................................Passed
    Late Hook................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn priority_fail_fast_stops_later_groups() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: fail-fast
                name: Failing Hook
                language: system
                entry: python3 -c "import sys; sys.exit(1)"
                always_run: true
                priority: 5
                fail_fast: true
              - id: sibling
                name: Same Priority Sibling
                language: system
                entry: python3 -c "import time; time.sleep(0.2)"
                always_run: true
                priority: 5
              - id: later
                name: Later Hook
                language: system
                entry: python3 -c "print('later ran')"
                always_run: true
                priority: 10
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Failing Hook.............................................................Failed
    - hook id: fail-fast
    - exit code: 1
    Same Priority Sibling....................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn explicitly_skipped_hook_does_not_affect_priority_group_outcome() {
    let context = TestEnv::new().with_file("file.txt", "hello\n").init_git();

    context
        .write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: skipped
                name: Skipped Check
                language: system
                entry: python3 -c "raise SystemExit(1)"
                always_run: true
                fail_fast: true
                priority: 0
              - id: modify
                name: Modifies File
                language: system
                entry: python3 -c "from pathlib import Path; p = Path('file.txt'); p.write_text(p.read_text() + 'x')"
                always_run: true
                priority: 0
              - id: later
                name: Later Hook
                language: system
                entry: python3 -c "print('later ran')"
                always_run: true
                priority: 10
    "#});

    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("--skip").arg("skipped"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Skipped Check...........................................................Skipped
    Modifies File............................................................Failed
    - hook id: modify
    - files were modified by this hook
    Later Hook...............................................................Passed

    ----- stderr -----
    "#);

    context.write_file("file.txt", "hello\n");
    cmd_snapshot!(context, context.run().arg("--skip").arg("skipped").arg("--hide-status").arg("failed"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Skipped Check...........................................................Skipped
    Later Hook...............................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn priority_group_modified_files_is_group_failure_and_output_is_indented() {
    // Read-only peers must not suppress the shared diff or the group's failure.
    let context = TestEnv::new().with_file("file.txt", "hello\n").init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: skipped
                name: Skipped Check
                language: system
                entry: python3 -c "raise SystemExit(1)"
                always_run: true
                priority: 0
              - id: modify
                name: Modifies File
                language: system
                entry: python3 -c "from pathlib import Path; p = Path('file.txt'); p.write_text(p.read_text() + 'x')"
                always_run: true
                verbose: true
                priority: 0
              - id: loud
                name: Prints Output
                language: system
                entry: python3 -c "print('hello from loud')"
                always_run: true
                verbose: true
                priority: 0
              - id: quiet
                name: No Output
                language: pygrep
                entry: not-present
                files: \.txt$
                always_run: true
                priority: 0
              - id: later
                name: Later Hook
                language: system
                entry: python3 -c "print('later ran')"
                always_run: true
                verbose: true
                priority: 10
    "#});

    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("--skip").arg("skipped"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Files were modified by following hooks...................................Failed
      ┌ Modifies File........................................................Passed
      │ - hook id: modify
      │ - duration: [TIME]
      │ Prints Output........................................................Passed
      │ - hook id: loud
      │ - duration: [TIME]
      │
      │ hello from loud
      └ No Output............................................................Passed
    Skipped Check...........................................................Skipped
    Later Hook...............................................................Passed
    - hook id: later
    - duration: [TIME]

      later ran

    ----- stderr -----
    ");

    context.write_file("file.txt", "hello\n");
    cmd_snapshot!(context, context.run().arg("--skip").arg("skipped").arg("--hide-status").arg("passed"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Files were modified by following hooks...................................Failed
    Skipped Check...........................................................Skipped

    ----- stderr -----
    "#);
}

/// Hook-level fail-fast lets every read-only group member run, even with one
/// worker, but stops before a later explicit priority group.
#[test]
fn fail_fast_stops_after_read_only_group() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: fail
                entry: Fixing files
                pass_filenames: false
                always_run: true
                fail_fast: false
              - id: trailing-whitespace
                name: trailing-whitespace
                language: fail
                entry: Fixing files
                pass_filenames: false
                always_run: true
                fail_fast: true
              - id: trailing-whitespace
                name: trailing-whitespace
                language: fail
                entry: Same group still runs
                pass_filenames: false
                always_run: true
              - id: trailing-whitespace
                name: trailing-whitespace
                language: fail
                entry: should not run
                priority: 10
                pass_filenames: false
                always_run: true
    "})
        .init_git();

    cmd_snapshot!(context, context.run().env("PREK_CONCURRENT_HOOKS", "1"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      Fixing files
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      Fixing files
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      Same group still runs

    ----- stderr -----
    ");
}

/// The CLI flag finishes the automatic read-only group before stopping;
/// without it, the later explicit priority group also runs.
#[test]
fn fail_fast_cli_flag() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                name: failing-hook
              - id: no-commit-to-branch
                name: passing-hook
                args: [--branch, protected]
              - id: no-commit-to-branch
                name: later-group
                args: [--branch, protected]
                priority: 10
    "})
        .init_git();

    cmd_snapshot!(context, context.run().env("PREK_CONCURRENT_HOOKS", "1"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    failing-hook.............................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'master'
    passing-hook.............................................................Passed
    later-group..............................................................Passed

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("PREK_CONCURRENT_HOOKS", "1").arg("--fail-fast"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    failing-hook.............................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'master'
    passing-hook.............................................................Passed

    ----- stderr -----
    ");
}

/// Project fail-fast finishes the automatic read-only group before stopping;
/// the CLI override also lets the later explicit priority group run.
#[test]
fn no_fail_fast_cli_flag() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        fail_fast: true
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                name: failing-hook
              - id: no-commit-to-branch
                name: passing-hook
                args: [--branch, protected]
              - id: no-commit-to-branch
                name: later-group
                args: [--branch, protected]
                priority: 10
    "})
        .init_git();

    cmd_snapshot!(context, context.run().env("PREK_CONCURRENT_HOOKS", "1"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    failing-hook.............................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'master'
    passing-hook.............................................................Passed

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().env("PREK_CONCURRENT_HOOKS", "1").arg("--no-fail-fast"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    failing-hook.............................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'master'
    passing-hook.............................................................Passed
    later-group..............................................................Passed

    ----- stderr -----
    ");
}
