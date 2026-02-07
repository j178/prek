mod common;

use crate::common::{TestContext, cmd_snapshot};

use assert_fs::fixture::{FileWriteStr, PathChild};

#[test]
fn self_repo_system_hook() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    cwd.child(".pre-commit-hooks.yaml")
        .write_str("- id: echo-hook\n  name: echo hook\n  entry: echo\n  language: system\n  files: \"\\\\.(txt|md)$\"\n")?;

    cwd.child("hello.txt").write_str("Hello\n")?;
    cwd.child("ignored.rs").write_str("fn main() {}\n")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: self
            hooks:
              - id: echo-hook
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo hook................................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn self_repo_with_overrides() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    cwd.child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r"
    - id: echo-hook
      name: echo hook
      entry: echo
      language: system
    "})?;

    cwd.child("hello.txt").write_str("Hello\n")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: self
            hooks:
              - id: echo-hook
                name: overridden name
                args: [--verbose]
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    overridden name..........................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn self_repo_missing_manifest() {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child("hello.txt")
        .write_str("Hello\n")
        .unwrap();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: self
            hooks:
              - id: my-hook
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Failed to read manifest of `self`
      caused by: failed to open file `[TEMP_DIR]/.pre-commit-hooks.yaml`: No such file or directory (os error 2)
    ");
}

#[test]
fn self_repo_unknown_hook_id() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();

    cwd.child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r"
    - id: real-hook
      name: real hook
      entry: echo
      language: system
    "})?;

    cwd.child("hello.txt").write_str("Hello\n")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: self
            hooks:
              - id: nonexistent-hook
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Hook `nonexistent-hook` not present in repo `self`
    ");

    Ok(())
}
