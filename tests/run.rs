use std::process::Command;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;
use insta::assert_snapshot;

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

    cmd_snapshot!(context.filters(), context.run(), @r#"
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
    "#);

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

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Hook `trailing-whitespace` is invalid
      caused by: Hook specified `additional_dependencies` `dotnet@6` but the language `dotnet` does not support installing dependencies for now
    "#);

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

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Hook `trailing-whitespace` is invalid
      caused by: Hook specified `language_version` `6` but the language `fail` does not install an environment
    "#);
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

    cmd_snapshot!(context.filters(), context.run(), @r#"
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
    "#);

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
    cmd_snapshot!(context.filters(), context.run().arg("nonexistent-hook"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: No hook found with id `nonexistent-hook` in stage `pre-commit`
    "#);

    // Multiple hook_ids match nothing
    cmd_snapshot!(context.filters(), context.run().arg("nonexistent-hook").arg("nonexistent-hook").arg("nonexistent-hook-2"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: No hooks found with ids `nonexistent-hook`, `nonexistent-hook-2` in stage `pre-commit`
    "#);

    // Hook-id matches one hook
    cmd_snapshot!(context.filters(), context.run().arg("hook2"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Second Hook..............................................................Passed

    ----- stderr -----
    "#);

    // Multiple hook-ids with mixed results (some exist, some don't)
    cmd_snapshot!(context.filters(), context.run().arg("hook1").arg("nonexistent").arg("hook2"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed
    Second Hook..............................................................Passed

    ----- stderr -----
    warning: Ignored non-existent hook ID: `nonexistent`
    "#);

    // Multiple valid hook-ids
    cmd_snapshot!(context.filters(), context.run().arg("hook1").arg("hook2").arg("nonexistent-hook"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    First Hook...............................................................Passed
    Second Hook..............................................................Passed

    ----- stderr -----
    warning: Ignored non-existent hook ID: `nonexistent-hook`
    "#);

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

/// `.pre-commit-config.yaml` is not staged.
#[test]
fn config_not_staged() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .touch()?;
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
    exit_code: 1
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

    cmd_snapshot!(context.filters(), context.run().env("SKIP", "end-of-file-fixer"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    fix end of files........................................................Skipped
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.run().env("SKIP", "trailing-whitespace,end-of-file-fixer"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace.....................................................Skipped
    fix end of files........................................................Skipped
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1

    ----- stderr -----
    "#);
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

    cmd_snapshot!(context.filters(), context.run(), @r#"
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
    "#);

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

    cmd_snapshot!(context.filters(), context.run(), @r#"
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
    "#);

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

    cmd_snapshot!(context.filters(), context.run(), @r#"
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
    "#);

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

    cmd_snapshot!(context.filters(), context.run(), @r#"
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
    "#);
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

    cmd_snapshot!(context.filters(), context.run().current_dir(&child).arg("--files").arg("file.txt"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    trailing-whitespace......................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
      foo/bar/baz/file.txt

    ----- stderr -----
    "#);

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

    cmd_snapshot!(context.filters(), context.run(), @r###"
    success: false
    exit_code: 1
    ----- stdout -----
    Pass environment.........................................................Failed
    - hook id: env-vars
    - exit code: 1
      1

    ----- stderr -----
    "###);
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

    cmd_snapshot!(filters, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    trailing-whitespace......................................................Passed
    - hook id: trailing-whitespace
    - duration: [TIME]
      Hello, world!

    ----- stderr -----
    Non-staged changes detected, saving to `[HOME]/patches/[TIME]-[PID].patch`

    Restored working tree changes from `[HOME]/patches/[TIME]-[PID].patch`
    "#);

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
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: You have unmerged paths. Resolve them before running prek
    "#);

    // Fix the conflict and run again.
    context.git_add(".");
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    trailing-whitespace......................................................Passed
    - hook id: trailing-whitespace
    - duration: [TIME]
      ['.pre-commit-config.yaml', 'file.txt']

    ----- stderr -----
    "#);

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

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    local-python-hook........................................................Failed
    - hook id: local-python-hook
    - exit code: 1
      Hello, world!

    ----- stderr -----
    "#);
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
    entry....................................................................
    ----- stderr -----
    error: Failed to run hook `entry`
      caused by: Hook `entry` is invalid
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
    error: Failed to initialize repo `https://notexistentatallnevergonnahappen.com/nonexistent/repo`
      caused by: command `git full clone` exited with an error:

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

    cmd_snapshot!(context.filters(), context.run().arg("--files").arg("non-exist-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    warning: This file does not exist, it will be ignored: `non-exist-files`
    "#);
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
    cmd_snapshot!(context.filters(), context.run().arg("--last-commit"), @r#"
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
    "#);

    // Now reset the files to their problematic state for comparison
    cwd.child("file1.txt").write_str("Hello, world!   \n")?; // trailing whitespace
    cwd.child("file3.txt").write_str("New file")?; // missing newline

    // Run with --all-files should check ALL files including file2.txt
    // This demonstrates that file2.txt was indeed filtered out in the previous test
    cmd_snapshot!(context.filters(), context.run().arg("--all-files"), @r#"
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
    "#);

    Ok(())
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
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]
      dir1/file.txt

    ----- stderr -----
    "#);

    // repeated `--directory`
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1").arg("--directory").arg("dir1"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]
      dir1/file.txt

    ----- stderr -----
    "#);

    // multiple `--directory`
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1").arg("--directory").arg("dir2"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]
      dir2/file.txt dir1/file.txt

    ----- stderr -----
    "#);

    // non-existing directory
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("non-existing-dir"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory............................................(no files to check)Skipped

    ----- stderr -----
    "#);

    // `--directory` with `--files`
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1").arg("--files").arg("dir1/file.txt"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]
      dir1/file.txt

    ----- stderr -----
    "#);
    cmd_snapshot!(context.filters(), context.run().arg("--directory").arg("dir1").arg("--files").arg("dir2/file.txt"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]
      dir2/file.txt dir1/file.txt

    ----- stderr -----
    "#);

    // run `--directory` inside a subdirectory
    cmd_snapshot!(context.filters(), context.run().current_dir(cwd.join("dir1")).arg("--directory").arg("."), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    directory................................................................Passed
    - hook id: directory
    - duration: [TIME]
      dir1/file.txt

    ----- stderr -----
    "#);

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
            r"current version `\d+\.\d+\.\d+`",
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
    cmd_snapshot!(context.filters(), context.run(), @r#"
  success: true
  exit_code: 0
  ----- stdout -----
  color....................................................................Passed
  - hook id: color
  - duration: [TIME]
    Hello, world!

  ----- stderr -----
  "#);

    // Force color output
    cmd_snapshot!(context.filters(), context.run().arg("--color=always"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    color....................................................................[42mPassed[49m
    [2m- hook id: color[0m
    [2m- duration: [TIME][0m
    [2m  [1;32mHello, world![0m[0m

    ----- stderr -----
    "#);

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

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    shebang-script...........................................................Passed
    - hook id: shebang-script
    - duration: [TIME]
      Hello, world!

    ----- stderr -----
    "#);

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
        .current_dir(cwd);

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"\[master \w{7}\]", r"[master COMMIT]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, commit, @r#"
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
    "#);

    Ok(())
}

/// Test hook id completion works.
#[cfg(unix)]
#[test]
fn completion() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hook
                name: a useful hook
                language: system
                entry: echo
                verbose: true
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    complete --keep-order --exclusive --command prek --arguments "(COMPLETE=fish "'[CURRENT_EXE]'" -- (commandline --current-process --tokenize --cut-at-cursor) (commandline --current-token))"

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg(""), @r#"
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
    gc	Clean unused cached repos
    clean	Clean out pre-commit files
    init-template-dir	Install hook script in a directory intended for use with `git config init.templateDir`
    try-repo	Try the pre-commit hooks in the current repo
    self	`prek` self management
    hook	a useful hook
    --all-files	Run on all files in the repo
    --files	Specific filenames to run hooks on
    --directory	Run hooks on all files in the specified directories
    --from-ref	The original ref in a `<from_ref>...<to_ref>` diff expression. Files changed in this diff will be run through the hooks
    --to-ref	The destination ref in a `from_ref...to_ref` diff expression. Defaults to `HEAD` if `from_ref` is specified
    --last-commit	Run hooks against the last commit. Equivalent to `--from-ref HEAD~1 --to-ref HEAD`
    --hook-stage	The stage during which the hook is fired
    --show-diff-on-failure	When hooks fail, run `git diff` directly afterward
    --config	Path to alternate config file
    --color	Whether to use color in output
    --help	Display the concise help for this command
    --no-progress	Hide all progress outputs
    --quiet	Do not print any output
    --verbose	Use verbose output
    --version	Display the prek version

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("run"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    run	Run hooks

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.run().env("COMPLETE", "fish").arg("--").arg("prek").arg("run").arg("h"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    hook	a useful hook

    ----- stderr -----
    "#);
}
