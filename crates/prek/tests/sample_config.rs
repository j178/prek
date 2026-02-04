use prek_consts::PRE_COMMIT_CONFIG_YAML;

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn sample_config() -> anyhow::Result<()> {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.sample_config(), @"
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

    insta::assert_snapshot!(context.read(PRE_COMMIT_CONFIG_YAML), @r##"
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
        rev: v6.0.0
        hooks:
          - id: trailing-whitespace
          - id: end-of-file-fixer
          - id: check-yaml
          - id: check-added-large-files
    "##);

    let child = context.work_dir().join("child");
    std::fs::create_dir(&child)?;

    cmd_snapshot!(context.filters(), context.sample_config().current_dir(&*child).arg("-f").arg("sample.yaml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `sample.yaml`

    ----- stderr -----
    "#);
    insta::assert_snapshot!(context.read("child/sample.yaml"), @r##"
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
    "##);

    Ok(())
}

#[test]
fn sample_config_toml() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.sample_config().arg("-f").arg("prek.toml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `prek.toml`

    ----- stderr -----
    "#);

    insta::assert_snapshot!(context.read("prek.toml"), @r#"
    #:schema: https://json.schemastore.org/prek.json
    # Configuration file for `prek`, a git hook framework written in Rust.
    # See https://prek.j178.dev for more information.

    [[repos]]
    repo = "builtin"
    hooks = [
        { id = "trailing-whitespace" },
        { id = "end-of-file-fixer" },
        { id = "check-added-large-files" },
    ]
    "#);
}

#[test]
fn sample_config_format() {
    let context = TestContext::new();

    cmd_snapshot!(context.filters(), context.sample_config().arg("--format").arg("toml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    #:schema: https://json.schemastore.org/prek.json
    # Configuration file for `prek`, a git hook framework written in Rust.
    # See https://prek.j178.dev for more information.

    [[repos]]
    repo = "builtin"
    hooks = [
        { id = "trailing-whitespace" },
        { id = "end-of-file-fixer" },
        { id = "check-added-large-files" },
    ]

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.sample_config().arg("--format").arg("yaml"), @"
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

    cmd_snapshot!(context.filters(), context.sample_config().arg("--format").arg("json"), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: invalid value 'json' for '--format <FORMAT>'
      [possible values: yaml, toml]

    For more information, try '--help'.
    ");
}

#[test]
fn respect_format() {
    let context = TestContext::new();

    // Write YAML format even with `.toml` extension.
    cmd_snapshot!(context.filters(), context.sample_config().arg("--format").arg("yaml").arg("-f").arg("prek.toml"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `prek.toml`

    ----- stderr -----
    ");

    insta::assert_snapshot!(context.read("prek.toml"), @"
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
}

#[test]
fn respect_format_if_filename_missing() {
    let context = TestContext::new();

    // Create `prek.toml` when TOML format is specified but filename is not given.
    cmd_snapshot!(context.filters(), context.sample_config().arg("--format").arg("toml").arg("-f"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `prek.toml`

    ----- stderr -----
    ");

    insta::assert_snapshot!(context.read("prek.toml"), @r#"
    #:schema: https://json.schemastore.org/prek.json
    # Configuration file for `prek`, a git hook framework written in Rust.
    # See https://prek.j178.dev for more information.

    [[repos]]
    repo = "builtin"
    hooks = [
        { id = "trailing-whitespace" },
        { id = "end-of-file-fixer" },
        { id = "check-added-large-files" },
    ]
    "#);
}
