use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn priorities_respected() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: late
                name: Late Hook
                language: system
                entry: python3 -c "print('late')"
                always_run: true
                priority: 10
              - id: early
                name: Early Hook
                language: system
                entry: python3 -c "print('early')"
                always_run: true
                priority: 0
              - id: middle
                name: Middle Hook
                language: system
                entry: python3 -c "print('middle')"
                always_run: true
                priority: 5
    "#})
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
        .with_config(indoc::indoc! {r#"
        priorities:
          early: 0
          late: 10
        repos:
          - repo: local
            hooks:
              - id: late
                name: Late Hook
                language: system
                entry: python3 -c "print('late')"
                always_run: true
                priority: late
              - id: early
                name: Early Hook
                language: system
                entry: python3 -c "print('early')"
                always_run: true
                priority: early
              - id: numeric
                name: Numeric Hook
                language: system
                entry: python3 -c "print('numeric')"
                always_run: true
                priority: 5
    "#})
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
                language: system
                entry: python3 -c "import time; time.sleep(0.1)"
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

/// Abort the run if a hook fails.
#[test]
fn fail_fast() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'print("Fixing files"); exit(1)'
                always_run: true
                fail_fast: false
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'print("Fixing files"); exit(1)'
                always_run: true
                fail_fast: true
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -V
                always_run: true
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -V
                always_run: true
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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

    ----- stderr -----
    ");
}

/// Test --fail-fast CLI flag stops execution after first failure.
#[test]
fn fail_fast_cli_flag() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: failing-hook
                name: failing-hook
                language: system
                entry: python3 -c 'print("Failed"); exit(1)'
                always_run: true
              - id: passing-hook
                name: passing-hook
                language: system
                entry: python3 -c 'print("Passed")'
                always_run: true
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    failing-hook.............................................................Failed
    - hook id: failing-hook
    - exit code: 1

      Failed
    passing-hook.............................................................Passed

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().arg("--fail-fast"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    failing-hook.............................................................Failed
    - hook id: failing-hook
    - exit code: 1

      Failed

    ----- stderr -----
    ");
}

/// Test --no-fail-fast CLI flag overrides config-level `fail_fast`.
#[test]
fn no_fail_fast_cli_flag() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        fail_fast: true
        repos:
          - repo: local
            hooks:
              - id: failing-hook
                name: failing-hook
                language: system
                entry: python3 -c 'print("Failed"); exit(1)'
                always_run: true
              - id: passing-hook
                name: passing-hook
                language: system
                entry: python3 -c 'print("Passed")'
                always_run: true
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    failing-hook.............................................................Failed
    - hook id: failing-hook
    - exit code: 1

      Failed

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().arg("--no-fail-fast"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    failing-hook.............................................................Failed
    - hook id: failing-hook
    - exit code: 1

      Failed
    passing-hook.............................................................Passed

    ----- stderr -----
    ");
}
