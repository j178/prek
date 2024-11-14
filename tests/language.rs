use crate::common::{cmd_snapshot, TestContext};
use anyhow::Result;
use assert_cmd::Command;
use assert_fs::prelude::*;

mod common;

#[test]
fn fail() -> Result<()> {
    let context = TestContext::new();

    context.init_project();

    let cwd = context.workdir();
    cwd.child("changelog").create_dir_all()?;
    cwd.child("changelog/changelog.md").touch()?;

    cwd.child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                - id: changelogs-rst
                  name: changelogs must be rst
                  entry: changelog filenames must end in .rst
                  language: fail
                  files: 'changelog/.*(?<!\.rst)$'
        "})?;

    Command::new("git")
        .current_dir(cwd)
        .arg("add")
        .arg(".")
        .assert()
        .success();

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    changelogs must be rst...................................................Failed
    - hook id: changelogs-rst
    - exit code: 1
    changelog filenames must end in .rst

    changelog/changelog.md

    ----- stderr -----
    "#);

    Ok(())
}
