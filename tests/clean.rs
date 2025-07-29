use assert_fs::assert::PathAssert;
use assert_fs::fixture::{PathChild, PathCreateDir};

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn clean() -> anyhow::Result<()> {
    let context = TestContext::new();

    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    cmd_snapshot!(context.filters(), context.clean().env("PREFLIGIT_HOME", &*home), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Cleaned `home`

    ----- stderr -----
    "#);

    home.assert(predicates::path::missing());

    Ok(())
}
