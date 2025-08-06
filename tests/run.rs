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

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("typos").arg("--hook-stage").arg("pre-push"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    No hook found for id `typos` and stage `pre-push`
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

#[test]
fn meta_hooks() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("file.txt").write_str("Hello, world!\n")?;
    cwd.child("valid.json").write_str("{}")?;
    cwd.child("invalid.json").write_str("{}")?;
    cwd.child("main.py").write_str(r#"print "abc"  "#)?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: check-hooks-apply
              - id: check-useless-excludes
              - id: identity
          - repo: local
            hooks:
              - id: match-no-files
                name: match no files
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                files: ^nonexistent$
              - id: useless-exclude
                name: useless exclude
                language: system
                entry: python3 -c 'import sys; sys.exit(0)'
                exclude: $nonexistent^
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Check hooks apply........................................................Failed
    - hook id: check-hooks-apply
    - exit code: 1
      match-no-files does not apply to this repository
    Check useless excludes...................................................Failed
    - hook id: check-useless-excludes
    - exit code: 1
      The exclude pattern "$nonexistent^" for useless-exclude does not match any files
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]
      file.txt
      .pre-commit-config.yaml
      valid.json
      invalid.json
      main.py
    match no files.......................................(no files to check)Skipped
    useless exclude..........................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn invalid_hook_id() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -V
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("invalid-hook-id"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    No hook found for id `invalid-hook-id` and stage `pre-commit`
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
    Your prefligit configuration file is not staged.
    Run `git add .pre-commit-config.yaml` to fix this.
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
    You have unmerged paths. Resolve them before running prefligit.
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

/// Supports reading `pre-commit-config.yml` as well.
#[test]
fn alternate_config_file() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python-hook
                name: local-python-hook
                language: python
                entry: python3 -c 'import sys; print("Hello, world!")'
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]
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
        .chain([(r"exit code: ", "exit status: ")])
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
    fatal: unable to access 'https://notexistentatallnevergonnahappen.com/nonexistent/repo/': Could not resolve host: notexistentatallnevergonnahappen.com
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

/// Test `prefligit run --directory` flags.
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
