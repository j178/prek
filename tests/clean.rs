use assert_fs::assert::PathAssert;
use assert_fs::fixture::{PathChild, PathCreateDir};

use crate::common::{cmd_snapshot, TestContext};

mod common;

#[test]
fn clean() -> anyhow::Result<()> {
    let context = TestContext::new();

    let home = context.workdir().child("home");
    home.create_dir_all()?;

    cmd_snapshot!(context.filters(), context.clean().env("PRE_COMMIT_HOME", &*home), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Cleaned `home`

    ----- stderr -----
    "#);

    home.assert(predicates::path::missing());

    Ok(())
}
