use std::path::Path;
use std::process::Command;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;
use insta::assert_snapshot;
use predicates::prelude::predicate;
use prek_consts::env_vars::EnvVars;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML, PREK_TOML};

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn run_basic() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: trailing-whitespace
              - id: end-of-file-fixer
              - id: check-json
    "});

    // Create a repository with some files.
    cwd.child("file.txt").write_str("Hello, world!\n")?;
    cwd.child("valid.json").write_str("{}")?;
    cwd.child("invalid.json").write_str("{}")?;
    cwd.child("main.py").write_str(r#"print "abc"  "#)?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing main.py
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1
    - files were modified by this hook

      Fixing valid.json
      Fixing invalid.json
      Fixing main.py
    check json...............................................................Passed

    ----- stderr -----
    ");

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("trailing-whitespace"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    trim trailing whitespace.................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn run_in_non_git_repo() {
    let context = TestContext::new();

    let mut filters = context.filters();
    filters.push((r"exit code: ", "exit status: "));

    cmd_snapshot!(filters, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Command `get git root` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: not a git repository (or any of the parent directories): .git
    ");
}

#[test]
fn invalid_config() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config("invalid: config");
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: missing field `repos`
    "#);

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: dotnet
                additional_dependencies: ["dotnet@6"]
                entry: echo Hello, world!
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `trailing-whitespace`
      caused by: Hook specified `additional_dependencies: dotnet@6` but the language `dotnet` does not support installing dependencies for now
    ");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: fail
                language_version: '6'
                entry: echo Hello, world!
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `trailing-whitespace`
      caused by: Hook specified `language_version: 6` but the language `fail` does not support toolchain installation for now
    ");
}

/// Use same repo multiple times, with same or different revisions.
#[test]
fn same_repo() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: trailing-whitespace
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: trailing-whitespace
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v4.6.0
            hooks:
              - id: trailing-whitespace
    "});

    cwd.child("file.txt").write_str("Hello, world!\n")?;
    cwd.child("valid.json").write_str("{}")?;
    cwd.child("invalid.json").write_str("{}")?;
    cwd.child("main.py").write_str(r#"print "abc"  "#)?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing main.py
    trim trailing whitespace.................................................Passed
    trim trailing whitespace.................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn local() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: system
                entry: echo Hello, world!
                always_run: true
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed

    ----- stderr -----
    "#);
}

/// Test multiple hook IDs scenarios.
#[test]
fn multiple_hook_ids() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hook1
                name: First Hook
                language: system
                entry: echo hook1
              - id: hook2
                name: Second Hook
                language: system
                entry: echo hook2
              - id: shared-name
                name: Shared Hook A
                language: system
                entry: echo shared-a
              - id: shared-name-2
                name: Shared Hook B
                language: system
                entry: echo shared-b
                alias: shared-name
    "});

    context.git_add(".");

    // Multiple repeated hook-id (should deduplicate)
    cmd_snapshot!(context.filters(), context.run().arg("hook1").arg("hook1").arg("hook1"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed

    ----- stderr -----
    "#);

    // Hook-id that matches multiple hooks (by alias)
    cmd_snapshot!(context.filters(), context.run().arg("shared-name"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Shared Hook A............................................................Passed
    Shared Hook B............................................................Passed

    ----- stderr -----
    "#);

    // Hook-id matches nothing
    cmd_snapshot!(context.filters(), context.run().arg("nonexistent-hook"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    warning: selector `nonexistent-hook` did not match any hooks
    error: No hooks found after filtering with the given selectors
    ");

    // Multiple hook_ids match nothing
    cmd_snapshot!(context.filters(), context.run().arg("nonexistent-hook").arg("nonexistent-hook").arg("nonexistent-hook-2"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    warning: the following selectors did not match any hooks or projects:
      - `nonexistent-hook`
      - `nonexistent-hook-2`
    error: No hooks found after filtering with the given selectors
    ");

    // Hook-id matches one hook
    cmd_snapshot!(context.filters(), context.run().arg("hook2"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Second Hook..............................................................Passed

    ----- stderr -----
    "#);

    // Multiple hook-ids with mixed results (some exist, some don't)
    cmd_snapshot!(context.filters(), context.run().arg("hook1").arg("nonexistent").arg("hook2"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed
    Second Hook..............................................................Passed

    ----- stderr -----
    warning: selector `nonexistent` did not match any hooks
    ");

    // Multiple valid hook-ids
    cmd_snapshot!(context.filters(), context.run().arg("hook1").arg("hook2").arg("nonexistent-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed
    Second Hook..............................................................Passed

    ----- stderr -----
    warning: selector `nonexistent-hook` did not match any hooks
    ");

    // Multiple hook-ids with some duplicates and aliases
    cmd_snapshot!(context.filters(), context.run().arg("hook1").arg("shared-name").arg("hook1"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed
    Shared Hook A............................................................Passed
    Shared Hook B............................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn priorities_respected() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
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
fn priority_fail_fast_stops_later_groups() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
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
fn priority_group_modified_files_is_group_failure_and_output_is_indented() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("file.txt").write_str("hello\n")?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
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

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
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
    Later Hook...............................................................Passed
    - hook id: later
    - duration: [TIME]

      later ran

    ----- stderr -----
    ");

    Ok(())
}

/// `.pre-commit-config.yaml` is not staged.
#[test]
fn config_not_staged() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.work_dir().child(PRE_COMMIT_CONFIG_YAML).touch()?;
    context.git_add(".");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -V
    "});

    cmd_snapshot!(context.filters(), context.run().arg("invalid-hook-id"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: prek configuration file is not staged, run `git add .pre-commit-config.yaml` to stage it
    "#);

    Ok(())
}

/// `.pre-commit-config.yaml` outside the repository should not be checked.
#[test]
fn config_outside_repo() -> Result<()> {
    let context = TestContext::new();

    // Initialize a git repository in ./work.
    let root = context.work_dir().child("work");
    root.create_dir_all()?;
    Command::new("git")
        .arg("init")
        .current_dir(&root)
        .assert()
        .success();

    // Create a configuration file in . (outside the repository).
    context
        .work_dir()
        .child("c.yaml")
        .write_str(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'print("Hello world")'
    "#})?;

    cmd_snapshot!(context.filters(), context.run().current_dir(&root).arg("-c").arg("../c.yaml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    trailing-whitespace..................................(no files to check)Skipped

    ----- stderr -----
    "#);

    Ok(())
}

/// Test the output format for a hook with a CJK name.
#[test]
fn cjk_hook_name() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
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
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    去除行尾空格.............................................................Passed
    fix end of files.........................................................Passed

    ----- stderr -----
    "#);
}

/// Skips hooks based on the `SKIP` environment variable.
#[test]
fn skips() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c "exit(1)"
              - id: end-of-file-fixer
                name: fix end of files
                language: system
                entry: python3 -c "exit(1)"
              - id: check-json
                name: check json
                language: system
                entry: python3 -c "exit(1)"
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().env("SKIP", "end-of-file-fixer"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().env("SKIP", "trailing-whitespace,end-of-file-fixer"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1

    ----- stderr -----
    ");
}

/// Run hooks with matched `stage`.
#[test]
fn stage() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: manual-stage
                name: manual-stage
                language: system
                entry: echo manual-stage
                stages: [ manual ]
              # Defaults to all stages.
              - id: default-stage
                name: default-stage
                language: system
                entry: echo default-stage
              - id: post-commit-stage
                name: post-commit-stage
                language: system
                entry: echo post-commit-stage
                stages: [ post-commit ]
    "});
    context.git_add(".");

    // By default, run hooks with `pre-commit` stage.
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage............................................................Passed

    ----- stderr -----
    "#);

    // Run hooks with `manual` stage.
    cmd_snapshot!(context.filters(), context.run().arg("--hook-stage").arg("manual"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    manual-stage.............................................................Passed
    default-stage............................................................Passed

    ----- stderr -----
    "#);

    // Run hooks with `post-commit` stage.
    cmd_snapshot!(context.filters(), context.run().arg("--hook-stage").arg("post-commit"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage........................................(no files to check)Skipped
    post-commit-stage....................................(no files to check)Skipped

    ----- stderr -----
    "#);
}

#[test]
fn fallback_to_manual_stage() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: manual-only
                name: manual-only
                language: system
                entry: echo manual-only
                stages: [ manual ]
              - id: another-manual
                name: another-manual
                language: system
                entry: echo another-manual
                stages: [ manual ]
              - id: default-stage
                name: default-stage
                language: system
                entry: echo default-stage
              - id: pre-push
                name: pre-push
                language: system
                entry: echo pre-push
                stages: [ pre-push ]
    "});
    context.git_add(".");

    // With pre-commit hooks present, default `prek run` stays on pre-commit.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage............................................................Passed

    ----- stderr -----
    ");

    // Explicit `--hook-stage pre-commit` keeps execution scoped to that stage.
    cmd_snapshot!(context.filters(), context.run().arg("--hook-stage").arg("pre-commit").arg("default-stage").arg("manual-only"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage............................................................Passed

    ----- stderr -----
    ");

    // Selecting manual + pre-commit hooks still runs only the pre-commit ones.
    cmd_snapshot!(context.filters(), context.run().arg("manual-only").arg("default-stage"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    default-stage............................................................Passed

    ----- stderr -----
    ");

    // Selecting only manual hooks should still succeed via fallback.
    cmd_snapshot!(context.filters(), context.run().arg("manual-only").arg("another-manual"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    manual-only..............................................................Passed
    another-manual...........................................................Passed

    ----- stderr -----
    ");

    // Mixing `pre-push` and manual selectors still runs the manual hook via fallback.
    cmd_snapshot!(context.filters(), context.run().arg("pre-push").arg("manual-only"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    manual-only..............................................................Passed

    ----- stderr -----
    ");
}

/// Test global `files`, `exclude`, and hook level `files`, `exclude`.
#[test]
fn files_and_exclude() -> Result<()> {
    let context = TestContext::new();

    context.init_project();

    let cwd = context.work_dir();
    cwd.child("file.txt").write_str("Hello, world!  \n")?;
    cwd.child("valid.json").write_str("{}\n  ")?;
    cwd.child("invalid.json").write_str("{}")?;
    cwd.child("main.py").write_str(r#"print "abc"  "#)?;

    // Global files and exclude.
    context.write_pre_commit_config(indoc::indoc! {r"
        files: file.txt
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: [text]
              - id: end-of-file-fixer
                name: fix end of files
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: [text]
              - id: check-json
                name: check json
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: [json]
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      ['file.txt']
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1

      ['file.txt']
    check json...........................................(no files to check)Skipped

    ----- stderr -----
    ");

    // Override hook level files and exclude.
    context.write_pre_commit_config(indoc::indoc! {r"
        files: file.txt
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                files: valid.json
              - id: end-of-file-fixer
                name: fix end of files
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                exclude: (valid.json|main.py)
              - id: check-json
                name: check json
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing whitespace..................................(no files to check)Skipped
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1

      ['file.txt']
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1

      ['file.txt']

    ----- stderr -----
    ");

    Ok(())
}

/// Test selecting files by type, `types`, `types_or`, and `exclude_types`.
#[test]
fn file_types() -> Result<()> {
    let context = TestContext::new();

    context.init_project();

    let cwd = context.work_dir();
    cwd.child("file.txt").write_str("Hello, world!  ")?;
    cwd.child("json.json").write_str("{}\n  ")?;
    cwd.child("main.py").write_str(r#"print "abc"  "#)?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: ["json"]
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types_or: ["json", "python"]
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                exclude_types: ["json"]
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                types: ["json" ]
                exclude_types: ["json"]
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      ['json.json']
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      ['main.py', 'json.json']
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      ['file.txt', '.pre-commit-config.yaml', 'main.py']
    trailing-whitespace..................................(no files to check)Skipped

    ----- stderr -----
    ");

    Ok(())
}

/// Abort the run if a hook fails.
#[test]
fn fail_fast() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
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
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
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

    cmd_snapshot!(context.filters(), context.run().arg("--fail-fast"), @r"
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

/// Run from a subdirectory. File arguments should be fixed to be relative to the root.
#[test]
fn subdirectory() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    let child = cwd.child("foo/bar/baz");
    child.create_dir_all()?;
    child.child("file.txt").write_str("Hello, world!\n")?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1]); exit(1)'
                always_run: true
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().current_dir(&child).arg("--files").arg("file.txt"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      foo/bar/baz/file.txt

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--cd").arg(&*child).arg("--files").arg("file.txt"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

      foo/bar/baz/file.txt

    ----- stderr -----
    ");

    Ok(())
}

/// Test hook `log_file` option.
#[test]
fn log_file() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'print("Fixing files"); exit(1)'
                always_run: true
                log_file: log.txt
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1

    ----- stderr -----
    "#);

    let log = context.read("log.txt");
    assert_eq!(log, "Fixing files");
}

/// Pass pre-commit environment variables to the hook.
#[test]
fn pass_env_vars() {
    let context = TestContext::new();

    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: env-vars
                name: Pass environment
                language: system
                entry: python3 -c "import os, sys; print(os.getenv('PRE_COMMIT')); sys.exit(1)"
                always_run: true
    "#});

    cmd_snapshot!(context.filters(), context.run(), @r"
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

#[test]
fn staged_files_only() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'print(open("file.txt", "rt").read())'
                verbose: true
                types: [text]
   "#});

    context
        .work_dir()
        .child("file.txt")
        .write_str("Hello, world!")?;
    context.git_add(".");

    // Non-staged files should be stashed and restored.
    context
        .work_dir()
        .child("file.txt")
        .write_str("Hello world again!")?;

    let filters: Vec<_> = context
        .filters()
        .into_iter()
        .chain([(r"/\d+-\d+.patch", "/[TIME]-[PID].patch")])
        .collect();

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    trailing-whitespace......................................................Passed
    - hook id: trailing-whitespace
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    Unstaged changes detected, stashing unstaged changes to `[HOME]/patches/[TIME]-[PID].patch`
    Restored working tree changes from `[HOME]/patches/[TIME]-[PID].patch`
    ");

    let content = context.read("file.txt");
    assert_snapshot!(content, @"Hello world again!");

    Ok(())
}

#[cfg(unix)]
#[test]
fn restore_on_interrupt() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    // The hook will sleep for 3 seconds.
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import time; open("out.txt", "wt").write(open("file.txt", "rt").read()); time.sleep(10)'
                verbose: true
                types: [text]
   "#});

    context
        .work_dir()
        .child("file.txt")
        .write_str("Hello, world!")?;
    context.git_add(".");

    // Non-staged files should be stashed and restored.
    context
        .work_dir()
        .child("file.txt")
        .write_str("Hello world again!")?;

    let mut child = context.run().spawn()?;
    let child_id = child.id();

    // Send an interrupt signal to the process.
    let handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        #[allow(clippy::cast_possible_wrap)]
        unsafe {
            libc::kill(child_id as i32, libc::SIGINT)
        };
    });

    handle.join().unwrap();
    child.wait()?;

    let content = context.read("out.txt");
    assert_snapshot!(content, @"Hello, world!");

    let content = context.read("file.txt");
    assert_snapshot!(content, @"Hello world again!");

    Ok(())
}

/// When in merge conflict, runs on files that have conflicts fixed.
#[test]
fn merge_conflicts() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a merge conflict.
    let cwd = context.work_dir();
    cwd.child("file.txt").write_str("Hello, world!")?;
    context.git_add(".");
    context.configure_git_author();
    context.git_commit("Initial commit");

    Command::new("git")
        .arg("checkout")
        .arg("-b")
        .arg("feature")
        .current_dir(cwd)
        .assert()
        .success();
    cwd.child("file.txt").write_str("Hello, world again!")?;
    context.git_add(".");
    context.git_commit("Feature commit");

    Command::new("git")
        .arg("checkout")
        .arg("master")
        .current_dir(cwd)
        .assert()
        .success();
    cwd.child("file.txt")
        .write_str("Hello, world from master!")?;
    context.git_add(".");
    context.git_commit("Master commit");

    Command::new("git")
        .arg("merge")
        .arg("feature")
        .current_dir(cwd)
        .assert()
        .code(1);

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sorted(sys.argv[1:]))'
                verbose: true
    "});

    // Abort on merge conflicts.
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: You have unmerged paths. Resolve them before running prek
    "#);

    // Fix the conflict and run again.
    context.git_add(".");
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    trailing-whitespace......................................................Passed
    - hook id: trailing-whitespace
    - duration: [TIME]

      ['.pre-commit-config.yaml', 'file.txt']

    ----- stderr -----
    ");

    Ok(())
}

/// Local python hook with no additional dependencies.
#[test]
fn local_python_hook() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python-hook
                name: local-python-hook
                language: python
                entry: python3 -c 'import sys; print("Hello, world!"); sys.exit(1)'
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
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
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: entry
                name: entry
                language: python
                entry: '"'
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `entry`
      caused by: Invalid hook `entry`
      caused by: Failed to parse entry `"` as commands
    "#);
}

/// Initialize a repo that does not exist.
#[test]
fn init_nonexistent_repo() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://notexistentatallnevergonnahappen.com/nonexistent/repo
            rev: v1.0.0
            hooks:
              - id: nonexistent
                name: nonexistent
        "});
    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"exit code: ", "exit status: "),
            // Normalize Git error message to handle environment-specific variations
            (
                r"fatal: unable to access 'https://notexistentatallnevergonnahappen\.com/nonexistent/repo/':.*",
                r"fatal: unable to access 'https://notexistentatallnevergonnahappen.com/nonexistent/repo/': [error]"
            ),
        ])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Failed to initialize repo `https://notexistentatallnevergonnahappen.com/nonexistent/repo`
      caused by: Command `git full clone` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: unable to access 'https://notexistentatallnevergonnahappen.com/nonexistent/repo/': [error]
    ");
}

/// Test hooks that specifies `types: [directory]`.
#[test]
fn types_directory() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: directory
                name: directory
                language: system
                entry: echo
                types: [directory]
        "});
    context.work_dir().child("dir").create_dir_all()?;
    context
        .work_dir()
        .child("dir/file.txt")
        .write_str("Hello, world!")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.run().arg("--files").arg("dir"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.run().arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.run().arg("--files").arg("non-exist-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    warning: This file does not exist and will be ignored: `non-exist-files`
    ");
    Ok(())
}

#[test]
fn run_last_commit() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: trailing-whitespace
              - id: end-of-file-fixer
    "});

    // Create initial files and make first commit
    cwd.child("file1.txt").write_str("Hello, world!\n")?;
    cwd.child("file2.txt")
        .write_str("Initial content with trailing spaces   \n")?; // This has issues but won't be in last commit
    context.git_add(".");
    context.git_commit("Initial commit");

    // Modify files and make second commit with trailing whitespace
    cwd.child("file1.txt").write_str("Hello, world!   \n")?; // trailing whitespace
    cwd.child("file3.txt").write_str("New file")?; // missing newline
    // Note: file2.txt is NOT modified in this commit, so it should be filtered out by --last-commit
    context.git_add(".");
    context.git_commit("Second commit with issues");

    // Run with --last-commit should only check files from the last commit
    // This should only process file1.txt and file3.txt, NOT file2.txt
    cmd_snapshot!(context.filters(), context.run().arg("--last-commit"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing file1.txt
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1
    - files were modified by this hook

      Fixing file3.txt

    ----- stderr -----
    ");

    // Now reset the files to their problematic state for comparison
    cwd.child("file1.txt").write_str("Hello, world!   \n")?; // trailing whitespace
    cwd.child("file3.txt").write_str("New file")?; // missing newline

    // Run with --all-files should check ALL files including file2.txt
    // This demonstrates that file2.txt was indeed filtered out in the previous test
    cmd_snapshot!(context.filters(), context.run().arg("--all-files"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing file1.txt
      Fixing file2.txt
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1
    - files were modified by this hook

      Fixing file3.txt

    ----- stderr -----
    ");

    Ok(())
}

/// Test `prek run --files` with multiple files.
#[test]
fn run_multiple_files() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: multiple-files
                name: multiple-files
                language: system
                entry: echo
                verbose: true
                types: [text]
    "});
    let cwd = context.work_dir();
    cwd.child("file1.txt").write_str("Hello, world!")?;
    cwd.child("file2.txt").write_str("Hello, world!")?;
    context.git_add(".");
    // `--files` with multiple files
    cmd_snapshot!(context.filters(), context.run().arg("--files").arg("file1.txt").arg("file2.txt"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    multiple-files...........................................................Passed
    - hook id: multiple-files
    - duration: [TIME]

      file2.txt file1.txt

    ----- stderr -----
    ");
    Ok(())
}

/// Test `prek run --files` with no files.
#[test]
fn run_no_files() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: no-files
                name: no-files
                language: system
                entry: echo
                verbose: true
    "});
    context.git_add(".");
    // `--files` with no files
    cmd_snapshot!(context.filters(), context.run().arg("--files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    no-files.................................................................Passed
    - hook id: no-files
    - duration: [TIME]

      .pre-commit-config.yaml

    ----- stderr -----
    ");
}

/// Test `prek run --directory` flags.
#[test]
fn run_directory() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: directory
                name: directory
                language: system
                entry: echo
                verbose: true
    "});

    let cwd = context.work_dir();
    cwd.child("dir1").create_dir_all()?;
    cwd.child("dir1/file.txt").write_str("Hello, world!")?;
    cwd.child("dir2").create_dir_all()?;
    cwd.child("dir2/file.txt").write_str("Hello, world!")?;
    context.git_add(".");

    // one `--directory`
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");

    // repeated `--directory`
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1").arg("--directory").arg("dir1"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");

    // multiple `--directory`
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1").arg("--directory").arg("dir2"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir2/file.txt dir1/file.txt

    ----- stderr -----
    ");

    // non-existing directory
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("non-existing-dir"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    ");

    // `--directory` with `--files`
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1").arg("--files").arg("dir1/file.txt"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1").arg("--files").arg("dir2/file.txt"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir2/file.txt dir1/file.txt

    ----- stderr -----
    ");

    // run `--directory` inside a subdirectory
    cmd_snapshot!(context.filters(), context.run().current_dir(cwd.join("dir1")).arg("--directory").arg("."), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--cd").arg("dir1").arg("--directory").arg("."), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]

      dir1/file.txt

    ----- stderr -----
    ");

    Ok(())
}

/// Test `minimum_prek_version` option.
#[test]
fn minimum_prek_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        minimum_prek_version: 10.0.0
        repos:
          - repo: local
            hooks:
              - id: directory
                name: directory
                language: system
                entry: echo
                verbose: true
    "});
    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(
            r"current version `\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?`",
            "current version `[CURRENT_VERSION]`",
        )])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: Required minimum prek version `10.0.0` is greater than current version `[CURRENT_VERSION]`. Please consider updating prek.
    "#);
}

/// Run hooks that would echo color.
#[test]
#[cfg(not(windows))]
fn color() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
      repos:
        - repo: local
          hooks:
            - id: color
              name: color
              language: python
              entry: python ./color.py
              verbose: true
              pass_filenames: false
  "});

    let script = indoc::indoc! {r"
      import sys
      if sys.stdout.isatty():
          print('\033[1;32mHello, world!\033[0m')
      else:
          print('Hello, world!')
  "};
    context.work_dir().child("color.py").write_str(script)?;

    context.git_add(".");

    // Run default. In integration tests, we don't have a TTY.
    // So this prints without color.
    cmd_snapshot!(context.filters(), context.run(), @r"
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
    cmd_snapshot!(context.filters(), context.run().arg("--color=always"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    color....................................................................[42mPassed[49m
    [2m- hook id: color[0m
    [2m- duration: [TIME][0m

      [1;32mHello, world![0m

    ----- stderr -----
    ");

    Ok(())
}

/// Test running hook whose `entry` is script with shebang on Windows.
#[test]
fn shebang_script() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a script with shebang.
    let script = indoc::indoc! {r"
        #!/usr/bin/env python
        import sys
        print('Hello, world!')
        sys.exit(0)
    "};
    context.work_dir().child("script.py").write_str(script)?;

    context.write_pre_commit_config(indoc::indoc! {r"
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
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    shebang-script...........................................................Passed
    - hook id: shebang-script
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    ");

    Ok(())
}

/// Test `git commit -a` works without `.git/index.lock exists` error.
#[test]
fn git_commit_a() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: echo
                name: echo
                language: system
                entry: echo
                verbose: true
    "});

    // Create a file and commit it.
    let cwd = context.work_dir();
    let file = cwd.child("file.txt");
    file.write_str("Hello, world!\n")?;

    cmd_snapshot!(context.filters(), context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    context.git_add(".");
    context.git_commit("Initial commit");

    // Edit the file
    file.write_str("Hello, world again!\n")?;

    let mut commit = Command::new("git");
    commit
        .arg("commit")
        .arg("-a")
        .arg("-m")
        .arg("Update file")
        .env(EnvVars::PREK_HOME, &**context.home_dir())
        .current_dir(cwd);

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"\[master \w{7}\]", r"[master COMMIT]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master COMMIT] Update file
     1 file changed, 1 insertion(+), 1 deletion(-)

    ----- stderr -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]

      file.txt
    ");

    Ok(())
}

fn write_pre_commit_config(path: &Path, hooks: &[(&str, &str)]) -> Result<()> {
    let mut yaml = String::from(indoc::indoc! {"
        repos:
          - repo: local
            hooks:
    "});
    for (id, name) in hooks {
        let hook = textwrap::indent(
            &indoc::formatdoc! {"
        - id: {}
          name: {}
          entry: echo
          language: system
        ", id, name
            },
            "      ",
        );
        yaml.push_str(&hook);
    }

    std::fs::create_dir_all(path)?;
    std::fs::write(path.join(PRE_COMMIT_CONFIG_YAML), yaml)?;

    Ok(())
}

#[cfg(unix)]
#[test]
fn selectors_completion() -> Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();

    // Root project with one hook
    write_pre_commit_config(cwd, &[("root-hook", "Root Hook")])?;

    // Nested project at app/ with one hook
    let app = cwd.join("app");
    write_pre_commit_config(&app, &[("app-hook", "App Hook")])?;

    // Deeper nested project at app/lib/ with one hook
    let app_lib = app.join("lib");
    write_pre_commit_config(&app_lib, &[("lib-hook", "Lib Hook")])?;

    // Unrelated non-project dir should not appear in subdir suggestions
    cwd.child("scratch").create_dir_all()?;

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg(""), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    install	Install the prek git hook
    install-hooks	Create hook environments for all hooks used in the config file
    run	Run hooks
    list	List available hooks
    uninstall	Uninstall the prek git hook
    validate-config	Validate `.pre-commit-config.yaml` files
    validate-manifest	Validate `.pre-commit-hooks.yaml` files
    sample-config	Produce a sample `.pre-commit-config.yaml` file
    auto-update	Auto-update pre-commit config to the latest repos' versions
    cache	Manage the prek cache
    init-template-dir	Install hook script in a directory intended for use with `git config init.templateDir`
    try-repo	Try the pre-commit hooks in the current repo
    self	`prek` self management
    app/
    app:
    app-hook	App Hook
    lib-hook	Lib Hook
    root-hook	Root Hook
    --skip	Skip the specified hooks or projects
    --all-files	Run on all files in the repo
    --files	Specific filenames to run hooks on
    --directory	Run hooks on all files in the specified directories
    --from-ref	The original ref in a `<from_ref>...<to_ref>` diff expression. Files changed in this diff will be run through the hooks
    --to-ref	The destination ref in a `from_ref...to_ref` diff expression. Defaults to `HEAD` if `from_ref` is specified
    --last-commit	Run hooks against the last commit. Equivalent to `--from-ref HEAD~1 --to-ref HEAD`
    --hook-stage	The stage during which the hook is fired
    --show-diff-on-failure	When hooks fail, run `git diff` directly afterward
    --fail-fast	Stop running hooks after the first failure
    --dry-run	Do not run the hooks, but print the hooks that would have been run
    --config	Path to alternate config file
    --cd	Change to directory before running
    --color	Whether to use color in output
    --refresh	Refresh all cached data
    --help	Display the concise help for this command
    --no-progress	Hide all progress outputs
    --quiet	Use quiet output
    --verbose	Use verbose output
    --log-file	Write trace logs to the specified file. If not specified, trace logs will be written to `$PREK_HOME/prek.log`
    --version	Display the prek version

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("ap"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/
    app:
    app-hook	App Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app:"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app:app-hook	App Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app:app"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app:app-hook	App Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/lib/
    app/lib:

    ----- stderr -----
    ");
    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app/li"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/lib/
    app/lib:

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app/lib:"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/lib:lib-hook	Lib Hook

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("app/lib/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    app/lib/

    ----- stderr -----
    ");

    Ok(())
}

/// Test reusing hook environments only when dependencies are exactly same. (ignore order)
#[test]
fn reuse_env() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
    repos:
      - repo: https://github.com/PyCQA/flake8
        rev: 7.1.1
        hooks:
          - id: flake8
            additional_dependencies: [flake8-errmsg]
    "});

    context
        .work_dir()
        .child("err.py")
        .write_str("raise ValueError('error')\n")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    flake8...................................................................Failed
    - hook id: flake8
    - exit code: 1

      err.py:1:1: EM101 Exceptions must not use a string literal; assign to a variable first

    ----- stderr -----
    ");

    // Remove dependencies, so the environment should not be reused.
    context.write_pre_commit_config(indoc::indoc! {r"
    repos:
      - repo: https://github.com/PyCQA/flake8
        rev: 7.1.1
        hooks:
          - id: flake8
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    flake8...................................................................Passed

    ----- stderr -----
    ");

    // There should be two hook environments.
    assert_eq!(context.home_dir().child("hooks").read_dir()?.count(), 2);

    Ok(())
}

#[test]
fn dry_run() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: fail
                name: fail
                entry: fail
                language: fail
    "});
    context.git_add(".");

    // Run with `--dry-run`
    cmd_snapshot!(context.filters(), context.run().arg("--dry-run").arg("-v"), @r"
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

/// Supports reading `pre-commit-config.yml` as well.
#[test]
fn alternate_config_file() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YML)
        .write_str(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python-hook
                name: local-python-hook
                language: python
                entry: python3 -c 'import sys; print("Hello, world!")'
    "#})?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    ");

    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YAML)
        .write_str(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python-hook
                name: local-python-hook
                language: python
                entry: python3 -c 'import sys; print("Hello, world!")'
    "#})?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("--refresh").arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    warning: Multiple configuration files found (`.pre-commit-config.yaml`, `.pre-commit-config.yml`); using `[TEMP_DIR]/.pre-commit-config.yaml`
    ");

    context
        .work_dir()
        .child(PREK_TOML)
        .write_str(indoc::indoc! {r#"
        [[repos]]
        repo = "local"
        hooks = [
          {
            id = "local-python-hook",
            name = "local-python-hook",
            language = "python",
            entry = "python3 -c 'import sys; print(\"Hello, world!\")'"
          }
        ]
    "#})?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("--refresh").arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    warning: Multiple configuration files found (`prek.toml`, `.pre-commit-config.yaml`, `.pre-commit-config.yml`); using `[TEMP_DIR]/prek.toml`
    ");

    Ok(())
}

/// Supports `prek.toml` as configuration file.
#[test]
fn prek_toml() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child(PREK_TOML)
        .write_str(indoc::indoc! {r#"
        [[repos]]
        repo = "local"
        hooks = [
          {
            id = "local-python-hook",
            name = "local-python-hook",
            language = "python",
            entry = "python3 -c 'import sys; print(\"Hello, world!\")'"
          }
        ]
    "#})?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn show_diff_on_failure() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.disable_auto_crlf();

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
    context.write_pre_commit_config(config);
    context
        .work_dir()
        .child("file.txt")
        .write_str("Original line\n")?;
    context.git_add(".");

    let mut filters = context.filters();
    filters.push((r"index \w{7}\.\.\w{7} \d{6}", "index [OLD]..[NEW] 100644"));

    // When failed in CI environment
    cmd_snapshot!(filters.clone(), context.run().env(EnvVars::CI, "1").arg("--show-diff-on-failure").arg("-v"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    modify...................................................................Failed
    - hook id: modify
    - duration: [TIME]
    - files were modified by this hook

    Hint: Some hooks made changes to the files.
    If you are seeing this message in CI, reproduce locally with: `prek run --all-files`
    To run prek as part of git workflow, use `prek install` to set up git hooks.

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

    context
        .work_dir()
        .child("file.txt")
        .write_str("Original line\n")?;
    context.git_add(".");
    // When failed in non-CI environment
    cmd_snapshot!(filters.clone(), context.run().env_remove(EnvVars::CI).arg("--show-diff-on-failure").arg("-v"), @r"
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
    let app = context.work_dir().child("app");
    app.create_dir_all()?;
    app.child("file.txt").write_str("Original line\n")?;
    app.child(PRE_COMMIT_CONFIG_YAML).write_str(config)?;

    Command::new("git")
        .arg("add")
        .arg(".")
        .current_dir(&app)
        .assert()
        .success();

    cmd_snapshot!(filters.clone(), context.run().env_remove(EnvVars::CI).current_dir(&app).arg("--show-diff-on-failure"), @r"
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

    context.git_add(".");

    // Run in the root
    // Since we add a new subproject, use `--refresh` to find that.
    cmd_snapshot!(filters.clone(), context.run().env_remove(EnvVars::CI).arg("--show-diff-on-failure").arg("--refresh"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Running hooks for `app`:
    modify...................................................................Failed
    - hook id: modify
    - files were modified by this hook

    Running hooks for `.`:
    modify...................................................................Failed
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
    ");

    Ok(())
}

#[test]
fn run_quiet() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
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
    "});
    context.git_add(".");

    // Run with `--quiet`, only print failed hooks.
    cmd_snapshot!(context.filters(), context.run().arg("--quiet"), @r"
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
    cmd_snapshot!(context.filters(), context.run().arg("-qq"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    ");
}

/// Test `prek run --log-file <file>` flag.
#[test]
fn run_log_file() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: fail
                name: fail
                entry: fail
                language: fail
    "});
    context.git_add(".");

    // Run with `--no-log-file`, no `prek.log` is created.
    cmd_snapshot!(context.filters(), context.run().arg("--no-log-file"), @r"
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
    cmd_snapshot!(context.filters(), context.run().arg("--log-file").arg("log"), @r"
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
        .work_dir()
        .child("log")
        .assert(predicate::path::exists());
}

/// Test `language_version: system` works and disables downloading.
#[test]
fn system_language_version() {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may not have toolchains installed locally.
        return;
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
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
   "});
    context.git_add(".");

    // Binaries can't be found, `system` must fail.
    cmd_snapshot!(
        context.filters(),
        context.run()
        .arg("system-node")
        .env(EnvVars::PREK_INTERNAL__GO_BINARY_NAME, "go-never-exist")
        .env(EnvVars::PREK_INTERNAL__NODE_BINARY_NAME, "node-never-exist"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `system-node`
      caused by: Failed to install node
      caused by: No suitable system Node version found and downloads are disabled
    ");

    cmd_snapshot!(
        context.filters(),
        context.run()
        .arg("system-go")
        .env(EnvVars::PREK_INTERNAL__GO_BINARY_NAME, "go-never-exist")
        .env(EnvVars::PREK_INTERNAL__NODE_BINARY_NAME, "node-never-exist"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `system-go`
      caused by: Failed to install go
      caused by: No suitable system Go version found and downloads are disabled
    ");

    // When binaries are available, hooks pass.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    system-node..............................................................Passed
    system-go................................................................Passed

    ----- stderr -----
    ");
}

/// Tests that empty `entry` field.
#[test]
fn empty_entry() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: python
                entry: ''
                pass_filenames: false
   "});
    context.git_add(".");

    // Go and Node can't be found, `system` must fail.
    cmd_snapshot!(context.filters(), context.run(), @r"
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
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-stdin
                name: check-stdin
                language: python
                entry: python -c 'import sys; sys.stdin.read(); print("STDIN closed")'
                pass_filenames: false
                verbose: true
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check-stdin..............................................................Passed
    - hook id: check-stdin
    - duration: [TIME]

      STDIN closed

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--color").arg("always"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check-stdin..............................................................[42mPassed[49m
    [2m- hook id: check-stdin[0m
    [2m- duration: [TIME][0m

      STDIN closed

    ----- stderr -----
    ");
}
