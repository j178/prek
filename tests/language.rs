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

#[test]
fn docker_image() -> Result<()> {
    let context = TestContext::new();

    context.init_project();

    let cwd = context.workdir();
    // test suit from https://github.com/super-linter/super-linter/tree/main/test/linters/gitleaks/bad
    cwd.child("gitleaks_bad_01.txt").write_str(
        r"aws_access_key_id = AROA47DSWDEZA3RQASWB
aws_secret_access_key = wQwdsZDiWg4UA5ngO0OSI2TkM4kkYxF6d2S1aYWM",
    )?;

    Command::new("docker")
        .args(["pull", "zricethezav/gitleaks:latest"])
        .assert()
        .success();

    cwd.child(".pre-commit-config.yaml")
        .write_str(indoc::indoc!
        // language=yaml
        {r"
            repos:
                - repo: local
                  hooks:
                      - id: gitleaks-docker
                        name: Detect hardcoded secrets
                        language: docker_image
                        entry: zricethezav/gitleaks:latest git --pre-commit --redact --staged --verbose
                        pass_filenames: false
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
    Detect hardcoded secrets.................................................Failed
    - hook id: gitleaks-docker
    - exit code: 1
      ○
          │╲
          │ ○
          ○ ░
          ░    gitleaks

      Finding:     aws_access_key_id = REDACTED
      Secret:      REDACTED
      RuleID:      generic-api-key
      Entropy:     3.521928
      File:        gitleaks_bad_01.txt
      Line:        1
      Fingerprint: gitleaks_bad_01.txt:generic-api-key:1

      Finding:     ...ROA47DSWDEZA3RQASWB
      aws_secret_access_key = REDACTED
      Secret:      REDACTED
      RuleID:      generic-api-key
      Entropy:     4.703056
      File:        gitleaks_bad_01.txt
      Line:        2
      Fingerprint: gitleaks_bad_01.txt:generic-api-key:2

      12:29PM INF 1 commits scanned.
      12:29PM INF scan completed in [TIME]ms
      12:29PM WRN leaks found: 2

    ----- stderr -----
    ");
    Ok(())
}
