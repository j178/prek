use assert_fs::fixture::{FileWriteStr, PathChild};
use prek_consts::PRE_COMMIT_CONFIG_YAML;

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn validate_config() -> anyhow::Result<()> {
    let context = TestContext::new();

    // No files to validate.
    cmd_snapshot!(context.filters(), context.validate_config(), @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    warning: No configs to check
    ");

    context.write_pre_commit_config(indoc::indoc! {r"
            repos:
              - repo: https://github.com/pre-commit/pre-commit-hooks
                rev: v5.0.0
                hooks:
                  - id: trailing-whitespace
                  - id: end-of-file-fixer
                  - id: check-json
        "});
    // Validate one file.
    cmd_snapshot!(context.filters(), context.validate_config().arg(PRE_COMMIT_CONFIG_YAML), @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    success: All configs are valid
    ");

    context
        .work_dir()
        .child("config-1.yaml")
        .write_str(indoc::indoc! {r"
            repos:
              - repo: https://github.com/pre-commit/pre-commit-hooks
        "})?;

    // Validate multiple files.
    cmd_snapshot!(context.filters(), context.validate_config().arg(PRE_COMMIT_CONFIG_YAML).arg("config-1.yaml"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `config-1.yaml`
      caused by: Invalid remote repo: missing field `rev`
    ");

    Ok(())
}

#[test]
fn validate_manifest() -> anyhow::Result<()> {
    let context = TestContext::new();

    // No files to validate.
    cmd_snapshot!(context.filters(), context.validate_manifest(), @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    warning: No manifests to check
    ");

    context
        .work_dir()
        .child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r"
            -   id: check-added-large-files
                name: check for added large files
                description: prevents giant files from being committed.
                entry: check-added-large-files
                language: python
                stages: [pre-commit, pre-push, manual]
                minimum_pre_commit_version: 3.2.0
        "})?;
    // Validate one file.
    cmd_snapshot!(context.filters(), context.validate_manifest().arg(".pre-commit-hooks.yaml"), @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    success: All manifests are valid
    ");

    context
        .work_dir()
        .child("hooks-1.yaml")
        .write_str(indoc::indoc! {r"
            -   id: check-added-large-files
                name: check for added large files
                description: prevents giant files from being committed.
                language: python
                stages: [pre-commit, pre-push, manual]
                minimum_pre_commit_version: 3.2.0
        "})?;

    // Validate multiple files.
    cmd_snapshot!(context.filters(), context.validate_manifest().arg(".pre-commit-hooks.yaml").arg("hooks-1.yaml"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `hooks-1.yaml`
      caused by: .[0]: missing field `entry` at line 1 column 5
    ");

    Ok(())
}

#[test]
fn unexpected_keys_warning() {
    let context = TestContext::new();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            unexpected_repo_key: some_value
            hooks:
              - id: test-hook
                name: Test Hook
                entry: echo test
                language: system
        unexpected_top_level_key: some_value
        another_unknown: test
        minimum_pre_commit_version: 1.0.0
    "});

    cmd_snapshot!(context.filters(), context.validate_config().arg(PRE_COMMIT_CONFIG_YAML), @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    warning: Ignored unexpected keys in `.pre-commit-config.yaml`: `another_unknown`, `unexpected_top_level_key`, `repos[0].unexpected_repo_key`
    success: All configs are valid
    ");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            unexpected_repo_key: some_value
            hooks:
              - id: test-hook
                name: Test Hook
                entry: echo test
                language: system
                unexpected_hook_key_1: some_value
                unexpected_hook_key_2: some_value
                unexpected_hook_key_3: some_value
                unexpected_hook_key_4: some_value
        unexpected_top_level_key: some_value
        another_unknown: test
        minimum_pre_commit_version: 1.0.0
    "});

    cmd_snapshot!(context.filters(), context.validate_config().arg(PRE_COMMIT_CONFIG_YAML), @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    warning: Ignored unexpected keys in `.pre-commit-config.yaml`:
      - `another_unknown`
      - `unexpected_top_level_key`
      - `repos[0].unexpected_repo_key`
      - `repos[0].hooks[0].unexpected_hook_key_1`
      - `repos[0].hooks[0].unexpected_hook_key_2`
      - `repos[0].hooks[0].unexpected_hook_key_3`
      - `repos[0].hooks[0].unexpected_hook_key_4`
    success: All configs are valid
    ");
}
