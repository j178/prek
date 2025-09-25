use assert_fs::assert::PathAssert;
use assert_fs::fixture::{PathChild, PathCreateDir};

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
