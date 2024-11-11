use crate::common::{cmd_snapshot, TestContext};

mod common;

#[test]
fn sample_config() {
    let context = TestContext::new();

    // Sample configuration.
    cmd_snapshot!(context.filters(), context.sample_config(), @r##"
    success: true
    exit_code: 0
    ----- stdout -----
    # See https://pre-commit.com for more information
    # See https://pre-commit.com/hooks.html for more hooks
    repos:
    -   repo: https://github.com/pre-commit/pre-commit-hooks
        rev: v5.0.0
        hooks:
        -   id: trailing-whitespace
        -   id: end-of-file-fixer
        -   id: check-yaml
        -   id: check-added-large-files

    ----- stderr -----
    "##);
}
