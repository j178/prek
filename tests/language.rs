use anyhow::Result;
use assert_fs::prelude::*;

use crate::common::{cmd_snapshot, TestContext};

mod common;

#[test]
fn fail() -> Result<()> {
    let context = TestContext::new();

    context.init_project();

    let cwd = context.workdir();
    cwd.child("changelog").create_dir_all()?;
    cwd.child("changelog/changelog.md").touch()?;

    context.write_pre_commit_config(indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                - id: changelogs-rst
                  name: changelogs must be rst
                  entry: changelog filenames must end in .rst
                  language: fail
                  files: 'changelog/.*(?<!\.rst)$'
        "});

    context.git_add(".");

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

#[test]
#[cfg(feature = "docker")]
fn docker() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/j178/pre-commit-docker-hooks
            rev: master
            hooks:
              - id: hello-world
                entry: "echo Hello, world!"
                always_run: true
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Cloning https://github.com/j178/pre-commit-docker-hooks@master
    Installing environment for https://github.com/j178/pre-commit-docker-hooks@master
    Hello World..............................................................Passed

    ----- stderr -----
    "#);
}
