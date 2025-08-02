use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn sample_config() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.sample_config(), @r##"
    success: true
    exit_code: 0
    ----- stdout -----
    # See https://pre-commit.com for more information
    # See https://pre-commit.com/hooks.html for more hooks
    repos:
      - repo: 'https://github.com/pre-commit/pre-commit-hooks'
        rev: v5.0.0
        hooks:
          - id: trailing-whitespace
          - id: end-of-file-fixer
          - id: check-yaml
          - id: check-added-large-files

    ----- stderr -----
    "##);

    cmd_snapshot!(context.filters(), context.sample_config().arg("-f"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `.pre-commit-config.yaml`

    ----- stderr -----
    "#);

    insta::assert_snapshot!(context.read(".pre-commit-config.yaml"), @r##"
    # See https://pre-commit.com for more information
    # See https://pre-commit.com/hooks.html for more hooks
    repos:
      - repo: 'https://github.com/pre-commit/pre-commit-hooks'
        rev: v5.0.0
        hooks:
          - id: trailing-whitespace
          - id: end-of-file-fixer
          - id: check-yaml
          - id: check-added-large-files
    "##);

    cmd_snapshot!(context.filters(), context.sample_config().arg("-f").arg("sample.yaml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `sample.yaml`

    ----- stderr -----
    "#);

    insta::assert_snapshot!(context.read("sample.yaml"), @r##"
    # See https://pre-commit.com for more information
    # See https://pre-commit.com/hooks.html for more hooks
    repos:
      - repo: 'https://github.com/pre-commit/pre-commit-hooks'
        rev: v5.0.0
        hooks:
          - id: trailing-whitespace
          - id: end-of-file-fixer
          - id: check-yaml
          - id: check-added-large-files
    "##);
}
