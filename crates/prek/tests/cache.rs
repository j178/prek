use assert_fs::assert::PathAssert;
use assert_fs::fixture::{PathChild, PathCreateDir};
use assert_fs::prelude::FileWriteStr;

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn cache_dir() {
    let context = TestContext::new();
    let home = context.work_dir().child("home");

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("dir").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [TEMP_DIR]/home

    ----- stderr -----
    ");
}

#[test]
fn cache_clean() -> anyhow::Result<()> {
    let context = TestContext::new();

    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("clean").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Cleaned `[TEMP_DIR]/home`

    ----- stderr -----
    ");

    home.assert(predicates::path::missing());

    // Test `prek clean` works for backward compatibility
    home.create_dir_all()?;
    cmd_snapshot!(context.filters(), context.command().arg("clean").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Cleaned `[TEMP_DIR]/home`

    ----- stderr -----
    ");

    home.assert(predicates::path::missing());

    Ok(())
}

#[test]
fn cache_size() -> anyhow::Result<()> {
    let context = TestContext::new().with_filtered_cache_size();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: end-of-file-fixer
    "});

    cwd.child("file.txt").write_str("Hello, world!\n")?;
    context.git_add(".");

    context.run();

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("size"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [SIZE]

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("size").arg("-H"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [SIZE]

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn cache_gc_removes_unreferenced_entries() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v6.0.0
            hooks:
              - id: check-yaml
          - repo: local
            hooks:
              - id: python-hook
                name: Python Hook
                entry: python -c "print('Hello from Python')"
                language: python
    "#});

    cwd.child("valid.yaml").write_str("a: 1\n")?;
    context.git_add(".");

    let home = context.home_dir();
    // Populate store + config tracking.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check yaml...............................................................Passed
    Python Hook..............................................................Passed

    ----- stderr -----
    ");

    // Add a few obviously-unused entries.
    home.child("repos/unused-repo").create_dir_all()?;
    home.child("hooks/unused-hook-env").create_dir_all()?;
    home.child("tools/node").create_dir_all()?;
    home.child("cache/go").create_dir_all()?;

    // Reduce hooks
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v6.0.0
            hooks:
              - id: check-yaml
    "});

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("gc"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repos, 2 hook envs, 1 tools, 2 caches

    ----- stderr -----
    ");

    home.child("repos/unused-repo")
        .assert(predicates::path::missing());
    home.child("hooks/unused-hook-env")
        .assert(predicates::path::missing());
    home.child("tools/node").assert(predicates::path::missing());
    home.child("cache/go").assert(predicates::path::missing());

    Ok(())
}
