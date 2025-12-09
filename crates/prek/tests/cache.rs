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

    let home = context.work_dir().child("home");

    home.create_dir_all()?;

    context.run();

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("size").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [SIZE]

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("size").arg("-H").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [SIZE]

    ----- stderr -----
    ");

    Ok(())
}
