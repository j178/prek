#[cfg(feature = "ci")]
use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn run_basic() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: trailing-whitespace
              - id: end-of-file-fixer
              - id: requirements-txt-fixer
                files: ^file\.txt$
              - id: check-json
    "})
        .with_files([
            ("file.txt", "z-project\na-project\n"),
            ("valid.json", "{}"),
            ("main.py", r#"print "abc"  "#),
        ])
        .init_git();

    cmd_snapshot!(context, context.run().arg("-q"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - description: trims trailing whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing main.py
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - description: ensures that a file is either empty, or ends with one newline
    - exit code: 1
    - files were modified by this hook

      Fixing valid.json
      Fixing main.py
    fix requirements.txt.....................................................Failed
    - hook id: requirements-txt-fixer
    - description: sorts entries in requirements.txt
    - exit code: 1
    - files were modified by this hook

      Sorting file.txt

    ----- stderr -----
    ");

    assert_eq!(context.read("file.txt"), "a-project\nz-project\n");

    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("trailing-whitespace"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    trim trailing whitespace.................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn fast_path_checks_filenames_from_entry_and_args() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-json
                entry: check-json from-entry.json
                args: [from-args.json]
                files: ^selected\.json$
    "})
        .with_files([
            ("from-entry.json", "invalid"),
            ("from-args.json", "invalid"),
            ("selected.json", "{}"),
        ])
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check json...............................................................Failed
    - hook id: check-json
    - description: checks json files for parseable syntax
    - exit code: 1

      from-entry.json: Failed to json decode (expected value at line 1 column 1)
      from-args.json: Failed to json decode (expected value at line 1 column 1)

    ----- stderr -----
    ");
}

#[test]
fn local() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: system
                entry: echo Hello, world!
                always_run: true
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed

    ----- stderr -----
    "#);
}

/// Pass pre-commit environment variables to the hook.
#[test]
fn pass_env_vars() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: env-vars
                name: Pass environment
                language: system
                entry: python3 -c "import os, sys; print(os.getenv('PRE_COMMIT')); sys.exit(1)"
                always_run: true
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Pass environment.........................................................Failed
    - hook id: env-vars
    - exit code: 1

      1

    ----- stderr -----
    ");
}

/// Local python hook with no additional dependencies.
#[test]
fn local_python_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python-hook
                name: local-python-hook
                language: python
                entry: python3 -c 'import sys; print("Hello, world!"); sys.exit(1)'
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    local-python-hook........................................................Failed
    - hook id: local-python-hook
    - exit code: 1

      Hello, world!

    ----- stderr -----
    ");
}

/// Invalid `entry`
#[test]
fn invalid_entry() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: entry
                name: entry
                language: python
                entry: '"'
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `entry`
      caused by: Invalid hook `entry`
      caused by: Failed to parse entry `"` as commands
    "#);
}

/// Test running hook whose `entry` is script with shebang on Windows.
#[test]
fn shebang_script() {
    let context = TestEnv::new().init_git();

    // Create a script with shebang.
    let script = indoc::indoc! {r"
        #!/usr/bin/env python
        import sys
        print('Hello, world!')
        sys.exit(0)
    "};
    let context = context
        .with_file("script.py", script)
        .with_config(indoc::indoc! {r"
      repos:
        - repo: local
          hooks:
            - id: shebang-script
              name: shebang-script
              language: python
              entry: script.py
              verbose: true
              pass_filenames: false
              always_run: true
    "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    shebang-script...........................................................Passed
    - hook id: shebang-script
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    ");
}

#[test]
fn dry_run() {
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

    // Run with `--dry-run`
    cmd_snapshot!(context, context.run().arg("--dry-run").arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fail....................................................................Dry Run
    - hook id: fail
    - duration: [TIME]

      `fail` would be run on 1 files:
      - .pre-commit-config.yaml

    ----- stderr -----
    ");
}

/// Test `language_version: system` works and disables downloading.
#[cfg(feature = "ci")]
#[test]
fn system_language_version() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: system-node
                name: system-node
                language: node
                language_version: system
                entry: node -v
                pass_filenames: false
              - id: system-go
                name: system-go
                language: golang
                language_version: system
                entry: go version
                pass_filenames: false
              - id: system-bun
                name: system-bun
                language: bun
                language_version: system
                entry: bun -e 'console.log(`Bun ${Bun.version}`)'
                pass_filenames: false
              - id: system-dotnet
                name: system-dotnet
                language: dotnet
                language_version: system
                entry: dotnet --version
                pass_filenames: false
       "})
        .init_git();

    // Binaries can't be found, `system` must fail.
    cmd_snapshot!(context,
        context.run()
        .arg("system-node")
        .env(EnvVars::PREK_INTERNAL__NODE_BINARY_NAME, "node-never-exist"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `system-node`
      caused by: Failed to install node
      caused by: No suitable Node version found for toolchain policy: managed (downloads disabled)
    "#);

    cmd_snapshot!(context,
        context.run()
        .arg("system-go")
        .env(EnvVars::PREK_INTERNAL__GO_BINARY_NAME, "go-never-exist"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `system-go`
      caused by: Failed to install go
      caused by: No suitable Go version found for toolchain policy: managed (downloads disabled)
    "#);

    cmd_snapshot!(context,
        context.run()
        .arg("system-bun")
        .env(EnvVars::PREK_INTERNAL__BUN_BINARY_NAME, "bun-never-exist"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `system-bun`
      caused by: Failed to install bun
      caused by: No suitable Bun version found for toolchain policy: managed (downloads disabled)
    "#);

    cmd_snapshot!(context,
        context.run()
        .arg("system-dotnet")
        .env(EnvVars::PREK_INTERNAL__DOTNET_BINARY_NAME, "dotnet-never-exist"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `system-dotnet`
      caused by: Failed to install dotnet SDK
      caused by: No suitable dotnet version found for toolchain policy: managed (downloads disabled)
    "#);
}

/// Tests that empty `entry` field.
#[test]
fn empty_entry() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: python
                entry: ''
                pass_filenames: false
       "})
        .init_git();

    // Go and Node can't be found, `system` must fail.
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `local`
      caused by: Invalid hook `local`
      caused by: Failed to parse entry: entry is empty
    ");
}

/// Test that hooks are run with stdin closed.
#[test]
fn run_with_stdin_closed() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-stdin
                name: check-stdin
                language: python
                entry: python -c 'import sys; sys.stdin.read(); print("STDIN closed"); sys.stdout.flush()'
                pass_filenames: false
                verbose: true
    "#}).init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check-stdin..............................................................Passed
    - hook id: check-stdin
    - duration: [TIME]

      STDIN closed

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().arg("--color").arg("always"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check-stdin[32m..............................................................[39m[32;7mPassed[0m
    [2m- hook id: check-stdin[0m
    [2m- duration: [TIME][0m

      STDIN closed

    ----- stderr -----
    "#);
}

/// `pass_filenames: n` limits each invocation to at most n files.
/// With n=1, each matched file gets its own invocation.
#[test]
fn pass_filenames_1_limits_batch_size() {
    let context = TestEnv::new().init_git();

    // Use a script that errors if it receives more than one filename argument.
    let context = context
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: one-at-a-time
                name: one at a time
                entry: python -c "import sys; args = sys.argv[1:]; sys.exit(0 if len(args) <= 1 else 1)"
                language: system
                pass_filenames: 1
                require_serial: true
                verbose: true
    "#})
        .with_files([("a.txt", "a"), ("b.txt", "b"), ("c.txt", "c")]);

    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    one at a time............................................................Passed
    - hook id: one-at-a-time
    - duration: [TIME]

    ----- stderr -----
    ");
}

/// `pass_filenames: n` limits each invocation to at most n files.
/// With n=2 and more than 2 matching files, multiple batches are spawned.
#[test]
fn pass_filenames_2_limits_batch_size() {
    let context = TestEnv::new().init_git();

    // Use a script that errors if it receives more than two filename arguments.
    let context = context
        .with_config(indoc::indoc! {r#"
            repos:
              - repo: local
                hooks:
                  - id: two-at-a-time
                    name: two at a time
                    entry: python -c "import sys; args = sys.argv[1:]; sys.exit(0 if len(args) <= 2 else 1)"
                    language: system
                    pass_filenames: 2
                    require_serial: true
                    verbose: true
        "#})
        .with_files([
            ("a.txt", "a"),
            ("b.txt", "b"),
            ("c.txt", "c"),
            ("d.txt", "d"),
            ("e.txt", "e"),
        ]);

    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    two at a time............................................................Passed
    - hook id: two-at-a-time
    - duration: [TIME]

    ----- stderr -----
    ");
}
