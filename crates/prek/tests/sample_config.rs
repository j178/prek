use prek_consts::PRE_COMMIT_CONFIG_YAML;

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn sample_config() -> anyhow::Result<()> {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.sample_config(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    # See https://pre-commit.com for more information
    # See https://pre-commit.com/hooks.html for more hooks
    repos:
      - repo: 'https://github.com/pre-commit/pre-commit-hooks'
        rev: v6.0.0
        hooks:
          - id: trailing-whitespace
          - id: end-of-file-fixer
          - id: check-yaml
          - id: check-added-large-files

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.sample_config().arg("-f"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `.pre-commit-config.yaml`

    ----- stderr -----
    "#);

    insta::assert_snapshot!(context.read(PRE_COMMIT_CONFIG_YAML), @r"
    # See https://pre-commit.com for more information
    # See https://pre-commit.com/hooks.html for more hooks
    repos:
      - repo: 'https://github.com/pre-commit/pre-commit-hooks'
        rev: v6.0.0
        hooks:
          - id: trailing-whitespace
          - id: end-of-file-fixer
          - id: check-yaml
          - id: check-added-large-files
    ");

    cmd_snapshot!(context.filters(), context.sample_config().arg("-f").arg("sample.yaml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `sample.yaml`

    ----- stderr -----
    "#);

    insta::assert_snapshot!(context.read("sample.yaml"), @r"
    # See https://pre-commit.com for more information
    # See https://pre-commit.com/hooks.html for more hooks
    repos:
      - repo: 'https://github.com/pre-commit/pre-commit-hooks'
        rev: v6.0.0
        hooks:
          - id: trailing-whitespace
          - id: end-of-file-fixer
          - id: check-yaml
          - id: check-added-large-files
    ");

    let child = context.work_dir().join("child");
    std::fs::create_dir(&child)?;

    cmd_snapshot!(context.filters(), context.sample_config().current_dir(&*child).arg("-f").arg("sample.yaml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `sample.yaml`

    ----- stderr -----
    "#);
    insta::assert_snapshot!(context.read("child/sample.yaml"), @r"
    # See https://pre-commit.com for more information
    # See https://pre-commit.com/hooks.html for more hooks
    repos:
      - repo: 'https://github.com/pre-commit/pre-commit-hooks'
        rev: v6.0.0
        hooks:
          - id: trailing-whitespace
          - id: end-of-file-fixer
          - id: check-yaml
          - id: check-added-large-files
    ");

    Ok(())
}
