use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn basic_after_ordering() {
    let context = TestContext::new();
    context.init_project();

    // Hook B should run before A because A has `after: [hook-b]`.
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: hook-a
                name: Hook A
                language: system
                entry: python3 -c "print('A ran')"
                always_run: true
                after: [hook-b]
              - id: hook-b
                name: Hook B
                language: system
                entry: python3 -c "print('B ran')"
                always_run: true
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Hook B...................................................................Passed
    Hook A...................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn group_after_fan_out() {
    let context = TestContext::new();
    context.init_project();

    // Two hooks in the "setup" group run in parallel first,
    // then "consumer" runs after all of them.
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: setup-a
                name: Setup A
                language: system
                entry: python3 -c "print('setup-a')"
                always_run: true
                group: setup
              - id: setup-b
                name: Setup B
                language: system
                entry: python3 -c "print('setup-b')"
                always_run: true
                group: setup
              - id: consumer
                name: Consumer
                language: system
                entry: python3 -c "print('consumer')"
                always_run: true
                after: ["group:setup"]
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Setup A..................................................................Passed
    Setup B..................................................................Passed
    Consumer.................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn independent_hooks_run_in_parallel() {
    let context = TestContext::new();
    context.init_project();

    // Three hooks with no `after` all run in the same wave (parallel).
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: hook-a
                name: Hook A
                language: system
                entry: python3 -c "print('A')"
                always_run: true
                group: all
              - id: hook-b
                name: Hook B
                language: system
                entry: python3 -c "print('B')"
                always_run: true
                group: all
              - id: hook-c
                name: Hook C
                language: system
                entry: python3 -c "print('C')"
                always_run: true
                group: all
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Hook A...................................................................Passed
    Hook B...................................................................Passed
    Hook C...................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn cycle_detection_error() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hook-a
                name: Hook A
                language: system
                entry: echo a
                always_run: true
                after: [hook-b]
              - id: hook-b
                name: Hook B
                language: system
                entry: echo b
                always_run: true
                after: [hook-a]
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Cycle detected in hook dependencies: hook-a, hook-b
    ");
}

#[test]
fn priority_and_after_mutually_exclusive() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hook-a
                name: Hook A
                language: system
                entry: echo a
                always_run: true
                priority: 5
                after: [hook-b]
              - id: hook-b
                name: Hook B
                language: system
                entry: echo b
                always_run: true
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `hook-a`
      caused by: `priority` and `group`/`after` are mutually exclusive on the same hook
    ");
}

#[test]
fn after_nonexistent_hook_error() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hook-a
                name: Hook A
                language: system
                entry: echo a
                always_run: true
                after: [nonexistent]
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Hook `hook-a` has `after: [nonexistent]` but no hook with id `nonexistent` exists
    "#);
}

#[test]
fn after_nonexistent_group_error() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: hook-a
                name: Hook A
                language: system
                entry: echo a
                always_run: true
                after: ["group:missing"]
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Hook `hook-a` has `after: [group:missing]` but no hook belongs to group `missing`
    "#);
}
