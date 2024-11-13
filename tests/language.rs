use crate::common::{cmd_snapshot, TestContext};
use anyhow::Result;
use assert_cmd::Command;
use assert_fs::prelude::*;

mod common;

#[test]
fn run_fail_language() -> Result<()> {
    let context = TestContext::new();

    let cwd = context.workdir();
    cwd.child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r"
            repos:
            -   repo: local
                hooks:
                -   id: changelogs-rst
                    name: changelogs must be rst
                    entry: changelog filenames must end in .rst
                    language: fail
                    files: 'changelog/.*\.rst$'
        "})?;

    // Create a repository with some files.
    let temp_dir = cwd.child("changelog");
    temp_dir.create_dir_all()?;
    temp_dir.child("changelog.rst").write_str("changelog")?;
    temp_dir.child("test.md").write_str("test")?;
    cwd.child("test.rst").write_str("Hello, world!\n")?;
    cwd.child("valid.json").write_str("{}")?;
    cwd.child("invalid.json").write_str("{}")?;
    cwd.child("main.py").write_str(r#"print "abc"  "#)?;

    Command::new("git")
        .current_dir(cwd)
        .arg("init")
        .assert()
        .success();
    Command::new("git")
        .current_dir(cwd)
        .arg("add")
        .arg(".")
        .assert()
        .success();

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    changelogs must be rst...................................................Failed
    - hook id: changelogs-rst
    - exit code: 1
    changelog filenames must end in .rst

    changelog/changelog.rst

    ----- stderr -----
    "#);

    Ok(())
}
