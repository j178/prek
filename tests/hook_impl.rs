use std::process::Command;
use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteFile, FileWriteStr, PathChild};
use indoc::indoc;
use common::TestContext;

use crate::common::cmd_snapshot;

mod common;

#[test]
fn hook_impl() -> Result<()> {
    let context = TestContext::new();

    context.init_project();

    context.workdir().child("pre-commit-config.yaml").write_str(indoc! { r#"
        repos:
        - repo: local
          hooks:
           - id: fail
             name: fail
             language: fail
             entry: always fail
    "#
    })?;

    Command::new("git").arg("add").arg(".").assert().success();

    let mut commit = Command::new("git");
    commit.arg("commit").arg("--allow-empty").arg("-m").arg("Initial commit");

    cmd_snapshot!(context.filters(), context.install(), @"");
    cmd_snapshot!(context.filters(), commit, @"");

    Ok(())
}
