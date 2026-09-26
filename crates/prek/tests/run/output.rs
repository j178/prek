use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;
use predicates::prelude::predicate;
use prek_consts::PRE_COMMIT_CONFIG_YAML;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn run_preserves_stdout_stderr_order() {
    let context = TestEnv::new()
        .with_file(
            "output.py",
            indoc::indoc! {r#"
            import os

            os.write(2, b"__PREK_STDERR_1__\n")
            os.write(1, b"__PREK_STDOUT_1__\n")
            os.write(2, b"__PREK_STDERR_2__\n")
            os.write(1, b"__PREK_STDOUT_2__\n")
        "#},
        )
        .init_git();

    context.write_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: output-order
                name: output-order
                language: system
                entry: python3 output.py
                always_run: true
                pass_filenames: false
                verbose: true
    "});
    context.git().add(".");

    cmd_snapshot!(context, context.run().args(["--all-files", "--color=never"]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    output-order.............................................................Passed
    - hook id: output-order
    - duration: [TIME]

      __PREK_STDERR_1__
      __PREK_STDOUT_1__
      __PREK_STDERR_2__
      __PREK_STDOUT_2__

    ----- stderr -----
    ");
}

#[test]
fn hook_details_include_first_description_line() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: quiet-failure
                name: Check project policy
                description: |-
                  Checks that the project follows its policy.
                  Additional details are not printed.
                language: system
                entry: python3 -c 'raise SystemExit(1)'
                always_run: true
                pass_filenames: false
              - id: verbose-success
                name: Verbose success
                description: This description is printed before the hook output.
                language: system
                entry: python3 -c 'print("specific output")'
                always_run: true
                pass_filenames: false
                verbose: true
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Check project policy.....................................................Failed
    - hook id: quiet-failure
    - description: Checks that the project follows its policy
    - exit code: 1
    Verbose success..........................................................Passed
    - hook id: verbose-success
    - description: This description is printed before the hook output
    - duration: [TIME]

      specific output

    ----- stderr -----
    ");
}

#[test]
fn run_displays_aliases_for_repeated_hook_ids() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: repeated
                alias: repeated-format
                name: Repeated hook
                language: system
                entry: git diff --quiet
                always_run: true
                pass_filenames: false
                verbose: true
              - id: repeated
                alias: repeated-lint
                name: Repeated hook
                language: system
                entry: git diff --quiet
                always_run: true
                pass_filenames: false
                verbose: true
    "})
        .init_git();

    cmd_snapshot!(context, context.run().arg("repeated"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Repeated hook............................................................Passed
    - hook id: repeated
    - hook alias: repeated-format
    - duration: [TIME]
    Repeated hook............................................................Passed
    - hook id: repeated
    - hook alias: repeated-lint
    - duration: [TIME]

    ----- stderr -----
    ");
}

/// Test the output format for a hook with a CJK name.
#[test]
fn cjk_hook_name() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: 去除行尾空格
                language: system
                entry: python3 -V
              - id: end-of-file-fixer
                name: fix end of files
                language: system
                entry: python3 -V
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    去除行尾空格.............................................................Passed
    fix end of files.........................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn hide_status_filters_hook_reports() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: pass
                name: Passing Hook
                language: system
                entry: python3 -c "print('passed output')"
                always_run: true
                verbose: true
              - id: fail
                name: Failing Hook
                language: system
                entry: python3 -c "import sys; print('failed output'); sys.exit(1)"
                always_run: true
              - id: skip
                name: Skipped Hook
                language: system
                entry: echo
                files: \.py$
    "#})
        .init_git();

    cmd_snapshot!(context, context.run().arg("--skip").arg("pass").arg("--hide-status").arg("passed,skipped"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Failing Hook.............................................................Failed
    - hook id: fail
    - exit code: 1

      failed output

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--hide-status").arg("failed"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Passing Hook.............................................................Passed
    - hook id: pass
    - duration: [TIME]

      passed output
    Skipped Hook.........................................(no files to check)Skipped

    ----- stderr -----
    "#);
}

#[test]
fn hide_status_uses_final_display_status() {
    let context = TestEnv::new().with_file("file.txt", "hello\n").init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: modify
                name: Modifies File
                language: system
                entry: python3 -c "from pathlib import Path; p = Path('file.txt'); p.write_text(p.read_text() + 'changed')"
                always_run: true
    "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("--hide-status").arg("passed"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Modifies File............................................................Failed
    - hook id: modify
    - files were modified by this hook

    ----- stderr -----
    "#);
}

#[test]
fn hidden_failed_status_still_writes_log_file() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: fail
                name: Failing Hook
                language: system
                entry: python3 -c "import sys; print('logged output'); sys.exit(1)"
                always_run: true
                log_file: hook.log
    "#})
        .init_git();

    context
        .run()
        .arg("--hide-status")
        .arg("failed")
        .assert()
        .failure();

    assert_eq!(context.read("hook.log"), "logged output");
}

/// Test hook `log_file` option.
#[test]
fn log_file() -> Result<()> {
    let context = TestEnv::new()
        .with_file(
            "config/.pre-commit-config.yaml",
            indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; sys.stdout.buffer.write(b"\x1b[2Kraw\xff"); exit(1)'
                always_run: true
                log_file: log.txt
        "#},
        )
        .init_git();
    let config_dir = context.child("config");
    let config_file = config_dir.child(PRE_COMMIT_CONFIG_YAML);
    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("-c").arg(config_file.path()), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

    ----- stderr -----
    "#);

    let log = fs_err::read(config_dir.join("log.txt"))?;
    assert_eq!(log, b"\x1b[2Kraw\xff");

    Ok(())
}

/// Run hooks that would echo color.
#[test]
#[cfg(not(windows))]
fn color() {
    let script = indoc::indoc! {r"
      import sys
      if sys.stdout.isatty():
          print('\033[1;32mHello, world!\033[0m')
      else:
          print('Hello, world!')
      sys.stdout.flush()
  "};
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
      repos:
        - repo: local
          hooks:
            - id: color
              name: color
              language: python
              entry: python ./color.py
              verbose: true
              pass_filenames: false
      "})
        .with_file("color.py", script)
        .init_git();

    // Run default. In integration tests, we don't have a TTY.
    // So this prints without color.
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    color....................................................................Passed
    - hook id: color
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    ");

    // Force color output
    cmd_snapshot!(context, context.run().arg("--color=always"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    color[32m....................................................................[39m[32;7mPassed[0m
    [2m- hook id: color[0m
    [2m- duration: [TIME][0m

      [1;32mHello, world![0m

    ----- stderr -----
    "#);
}

#[test]
#[cfg(not(windows))]
fn tty_output_preserves_color_without_replaying_terminal_controls() {
    let script = indoc::indoc! {r"
      import sys

      sys.stdout.write('discarded\r')
      sys.stdout.write('\033[2K')
      sys.stdout.write('\033[1;32mgreen\033[0m\n')
      sys.stdout.write('\033[1A\033[1B')
      sys.stdout.write('\033]0;title\007plain\n')
      sys.stdout.flush()
  "};
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
      repos:
        - repo: local
          hooks:
            - id: terminal-output
              name: terminal-output
              language: python
              entry: python ./terminal_output.py
              verbose: true
              pass_filenames: false
      "})
        .with_file("terminal_output.py", script)
        .init_git();

    cmd_snapshot!(context,
        context.run().arg("--color=always"),
        @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    terminal-output[32m..........................................................[39m[32;7mPassed[0m
    [2m- hook id: terminal-output[0m
    [2m- duration: [TIME][0m

      [1;32mgreen[0m
      plain

    ----- stderr -----
    "#
    );
}

#[test]
fn show_diff_on_failure() {
    let context = TestEnv::new()
        .with_filter(r"index \w{7}\.\.\w{7} \d{6}", "index [OLD]..[NEW] 100644")
        .init_git();

    let config = indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: modify
                name: modify
                language: python
                entry: python -c "import sys; open('file.txt', 'a').write('Added line\n')"
                pass_filenames: false
    "#};
    let context = context
        .with_config(config)
        .with_file("file.txt", "Original line\n");

    context.git().add(".");

    // When failed in CI environment
    cmd_snapshot!(context, context.run().env(EnvVars::CI, "1").arg("--show-diff-on-failure").arg("-v"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    modify...................................................................Failed
    - hook id: modify
    - duration: [TIME]
    - files were modified by this hook

    hint: Some hooks made changes to the files.
    If you are seeing this message in CI, reproduce locally with: `prek run --all-files`
    To run prek as part of Git workflow, use `prek install` to set up Git shims.

    All changes made by hooks:
    diff --git a/file.txt b/file.txt
    index [OLD]..[NEW] 100644
    --- a/file.txt
    +++ b/file.txt
    @@ -1 +1,2 @@
     Original line
    +Added line

    ----- stderr -----
    ");

    context.write_file("file.txt", "Original line\n");
    context.git().add(".");
    // When failed in non-CI environment
    cmd_snapshot!(context, context.run().env_remove(EnvVars::CI).arg("--show-diff-on-failure").arg("-v"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    modify...................................................................Failed
    - hook id: modify
    - duration: [TIME]
    - files were modified by this hook
    All changes made by hooks:
    diff --git a/file.txt b/file.txt
    index [OLD]..[NEW] 100644
    --- a/file.txt
    +++ b/file.txt
    @@ -1 +1,2 @@
     Original line
    +Added line

    ----- stderr -----
    ");

    // Run in the `app` subproject.
    let app = context.child("app");
    context.write_file("app/file.txt", "Original line\n");
    context.write_file("app/.pre-commit-config.yaml", config);

    context.git_at(&app).add(".");

    cmd_snapshot!(context, context.run().env_remove(EnvVars::CI).current_dir(&app).arg("--show-diff-on-failure"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    modify...................................................................Failed
    - hook id: modify
    - files were modified by this hook
    All changes made by hooks:
    diff --git a/app/file.txt b/app/file.txt
    index [OLD]..[NEW] 100644
    --- a/app/file.txt
    +++ b/app/file.txt
    @@ -1 +1,2 @@
     Original line
    +Added line

    ----- stderr -----
    ");

    context.git().add(".");

    // Run in the root
    // Since we add a new subproject, use `--refresh` to find that.
    cmd_snapshot!(context, context.run().env_remove(EnvVars::CI).arg("--show-diff-on-failure").arg("--refresh"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    × app
      modify.................................................................Failed
      - hook id: modify
      - files were modified by this hook
    × <workspace>
      modify.................................................................Failed
      - hook id: modify
      - files were modified by this hook
    All changes made by hooks:
    diff --git a/app/file.txt b/app/file.txt
    index [OLD]..[NEW] 100644
    --- a/app/file.txt
    +++ b/app/file.txt
    @@ -1,2 +1,3 @@
     Original line
     Added line
    +Added line
    diff --git a/file.txt b/file.txt
    index [OLD]..[NEW] 100644
    --- a/file.txt
    +++ b/file.txt
    @@ -1,2 +1,3 @@
     Original line
     Added line
    +Added line

    ----- stderr -----
    "#);
}

#[test]
fn run_quiet() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: success
                name: success
                entry: echo
                language: system
              - id: fail
                name: fail
                entry: fail
                language: fail
    "})
        .init_git();

    // Run with `--quiet`, only print failed hooks.
    cmd_snapshot!(context, context.run().arg("--quiet"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1

      fail

      .pre-commit-config.yaml

    ----- stderr -----
    ");

    // Run with `-qq`, do not print anything.
    cmd_snapshot!(context, context.run().arg("-qq"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    ");
}

/// Test `PREK_QUIET` environment variable.
#[test]
fn run_quiet_env() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: success
                name: success
                entry: echo
                language: system
              - id: fail
                name: fail
                entry: fail
                language: fail
    "})
        .init_git();

    // Run with `PREK_QUIET=1`, only print failed hooks.
    cmd_snapshot!(context, context.run().env(EnvVars::PREK_QUIET, "1"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1

      fail

      .pre-commit-config.yaml

    ----- stderr -----
    ");

    // Run with `PREK_QUIET=2`, does not print anything (silent mode).
    cmd_snapshot!(context, context.run().env(EnvVars::PREK_QUIET, "2"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    ");
}

/// Test `prek run --log-file <file>` flag.
#[test]
fn run_log_file() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: fail
                name: fail
                entry: fail
                language: fail
    "})
        .init_git();

    // Run with `--no-log-file`, no `prek.log` is created.
    cmd_snapshot!(context, context.run().arg("--no-log-file"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1

      fail

      .pre-commit-config.yaml

    ----- stderr -----
    ");
    context
        .home_dir()
        .child("prek.log")
        .assert(predicate::path::missing());

    // Write log to `log`.
    cmd_snapshot!(context, context.run().arg("--log-file").arg("log"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1

      fail

      .pre-commit-config.yaml

    ----- stderr -----
    ");
    context.child("log").assert(predicate::path::exists());
}

/// Test `prek --version` outputs version info.
#[test]
fn version_info() {
    // skip if not built in the git repository
    if option_env!("PREK_COMMIT_HASH").is_none() {
        return;
    }
    let context = TestEnv::new().with_filter(
        r"prek \d+\.\d+\.\d+(-[0-9A-Za-z]+(\.[0-9A-Za-z]+)*)?(\+\d+)? \(\w{9} [\d\-T:\.]+\)",
        "prek [CURRENT_VERSION] ([COMMIT] [DATE])",
    );
    cmd_snapshot!(context, context.command().arg("--version"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prek [CURRENT_VERSION] ([COMMIT] [DATE])

    ----- stderr -----
    ");
}
