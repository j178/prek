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

#[test]
fn docker() -> Result<()> {
    // spellchecker:off
    let context = TestContext::new();

    context.init_project();

    let cwd = context.workdir();
    // test suit from https://github.com/crate-ci/typos/blob/master/crates/typos-cli/tests/cmd/extend-words-case.in/file.txt
    cwd.child("file.txt").write_str(
        "public function noErrorOnTraillingSemicolonAndWhitespace(Connection $connection)",
    )?;
    cwd.child("_typos.toml")
        // language=toml
        .write_str(
            r#"
            [default.extend-words]
            "trailling" = "trailing"
            "#,
        )?;

    cwd.child(".pre-commit-config.yaml")
        .write_str(indoc::indoc!
        // language=yaml
        {r"
            repos:
              - repo: https://github.com/crate-ci/typos
                rev: v1.26.0
                hooks:
                  - id: typos-docker
                    args: []
        "})?;

    Command::new("git")
        .current_dir(cwd)
        .arg("add")
        .arg(".")
        .assert()
        .success();

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Cloning https://github.com/crate-ci/typos@v1.26.0
    typos....................................................................Failed
    - hook id: typos-docker
    - exit code: 2
      error: `Trailling` should be `Trailing`
        --> file.txt:1:26
        |
      1 | public function noErrorOnTraillingSemicolonAndWhitespace(Connection $connection)
        |                          ^^^^^^^^^
        |

    ----- stderr -----
    ");
    // spellchecker:on
    Ok(())
}
