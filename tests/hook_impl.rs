use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteStr, PathChild};
use common::TestContext;
use indoc::indoc;
use std::process::Command;

use crate::common::cmd_snapshot;

mod common;

#[test]
fn hook_impl() -> Result<()> {
    let context = TestContext::new();

    context.init_project();

    context
        .workdir()
        .child(".pre-commit-config.yaml")
        .write_str(indoc! { r"
            repos:
            - repo: local
              hooks:
               - id: fail
                 name: fail
                 language: fail
                 entry: always fail
                 always_run: true
            "
        })?;

    Command::new("git")
        .arg("add")
        .current_dir(context.workdir())
        .arg(".")
        .assert()
        .success();

    let mut commit = Command::new("git");
    commit
        .arg("commit")
        .current_dir(context.workdir())
        .arg("-m")
        .arg("Initial commit");

    cmd_snapshot!(context.filters(), context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pre-commit installed at .git/hooks/pre-commit

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), commit, @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1
    always fail

    .pre-commit-config.yaml
    "#);

    Ok(())
}
