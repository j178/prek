use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild};
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML, PREK_TOML};

use crate::common::{TestContext, cmd_snapshot};

mod common;

const YAML_CONFIG: &str = r#"
fail_fast: true
default_install_hook_types: [pre-push]
exclude: |
  (?x)^(
    .*/(snapshots)/.*|
  )$

repos:
  - repo: builtin
    hooks:
      - id: trailing-whitespace
      - id: mixed-line-ending
      - id: check-yaml
      - id: check-toml
      - id: end-of-file-fixer

  - repo: https://github.com/crate-ci/typos
    rev: v1.42.3
    hooks:
      - id: typos

  - repo: https://github.com/executablebooks/mdformat
    rev: '1.0.0'
    hooks:
      - id: mdformat
        language: python  # ensures that Renovate can update additional_dependencies
        args: [--number, --compact-tables, --align-semantic-breaks-in-lists]
        env:
          Hello: World
        priority: 1
        additional_dependencies:
          - mdformat-mkdocs==5.1.4
          - mdformat-simple-breaks==0.1.0

  - repo: local
    hooks:
      - id: taplo-fmt
        name: taplo fmt
        env:
          EnvVar: Value
          AnotherEnvVar: AnotherValue
        entry: taplo fmt --config .config/taplo.toml
        language: python
        additional_dependencies: ["taplo==0.9.3"]
        types: [toml]
"#;

#[test]
fn yaml_to_toml_writes_default_output() -> anyhow::Result<()> {
    let context = TestContext::new();

    context
        .work_dir()
        .child("config.yaml")
        .write_str(YAML_CONFIG)?;

    cmd_snapshot!(
        context.filters(),
        context
            .command()
            .args(["util", "yaml-to-toml", "config.yaml"]),
        @"
    success: true
    exit_code: 0
    ----- stdout -----
    Converted `config.yaml` → `prek.toml`

    ----- stderr -----
    "
    );

    insta::assert_snapshot!(context.read(PREK_TOML), @r#"
    # Configuration file for `prek`, a git hook framework written in Rust.
    # See https://prek.j178.dev for more information.
    #:schema https://www.schemastore.org/prek.json

    fail_fast = true
    default_install_hook_types = ["pre-push"]
    exclude = """
    (?x)^(
      .*/(snapshots)/.*|
    )$
    """

    [[repos]]
    repo = "builtin"
    hooks = [
      { id = "trailing-whitespace" },
      { id = "mixed-line-ending" },
      { id = "check-yaml" },
      { id = "check-toml" },
      { id = "end-of-file-fixer" }
    ]

    [[repos]]
    repo = "https://github.com/crate-ci/typos"
    rev = "v1.42.3"
    hooks = [
      { id = "typos" }
    ]

    [[repos]]
    repo = "https://github.com/executablebooks/mdformat"
    rev = "1.0.0"
    hooks = [
      {
        id = "mdformat",
        language = "python",
        args = [
          "--number",
          "--compact-tables",
          "--align-semantic-breaks-in-lists"
        ],
        env = { Hello = "World" },
        priority = 1,
        additional_dependencies = [
          "mdformat-mkdocs==5.1.4",
          "mdformat-simple-breaks==0.1.0"
        ]
      }
    ]

    [[repos]]
    repo = "local"
    hooks = [
      {
        id = "taplo-fmt",
        name = "taplo fmt",
        env = {
          EnvVar = "Value",
          AnotherEnvVar = "AnotherValue"
        },
        entry = "taplo fmt --config .config/taplo.toml",
        language = "python",
        additional_dependencies = ["taplo==0.9.3"],
        types = ["toml"]
      }
    ]
    "#);

    Ok(())
}

#[test]
fn yaml_to_toml_force_overwrite() -> anyhow::Result<()> {
    let context = TestContext::new();

    context
        .work_dir()
        .child("config.yaml")
        .write_str(YAML_CONFIG)?;
    context.work_dir().child(PREK_TOML).write_str("existing")?;

    cmd_snapshot!(
        context.filters(),
        context
            .command()
            .args(["util", "yaml-to-toml", "config.yaml"]),
        @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: File `prek.toml` already exists (use `--force` to overwrite)
    "
    );

    cmd_snapshot!(
        context.filters(),
        context
            .command()
            .args(["util", "yaml-to-toml", "config.yaml", "--force"]),
        @"
    success: true
    exit_code: 0
    ----- stdout -----
    Converted `config.yaml` → `prek.toml`

    ----- stderr -----
    "
    );

    Ok(())
}

#[test]
fn yaml_to_toml_rejects_invalid_config() -> anyhow::Result<()> {
    let context = TestContext::new();

    context
        .work_dir()
        .child("config.yaml")
        .write_str("repos: 123")?;

    cmd_snapshot!(
      context.filters(),
      context
        .command()
        .args(["util", "yaml-to-toml", "config.yaml"]),
      @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `config.yaml`
      caused by: error: line 1 column 8: unexpected event: expected sequence start
     --> <input>:1:8
      |
    1 | repos: 123
      |        ^ unexpected event: expected sequence start
    "
    );

    Ok(())
}

#[test]
fn yaml_to_toml_same_output() -> anyhow::Result<()> {
    let context = TestContext::new();

    context
        .work_dir()
        .child("config.yaml")
        .write_str(YAML_CONFIG)?;

    cmd_snapshot!(
        context.filters(),
        context
            .command()
            .args(["util", "yaml-to-toml", "config.yaml", "--output", "config.yaml"]),
        @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Output path `config.yaml` matches input; choose a different output path
    "
    );

    context
        .work_dir()
        .child(PREK_TOML)
        .assert(predicates::path::missing());

    Ok(())
}

#[test]
fn yaml_to_toml_discovers_pre_commit_config_yaml() -> anyhow::Result<()> {
    let context = TestContext::new();

    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YAML)
        .write_str(YAML_CONFIG)?;

    cmd_snapshot!(
        context.filters(),
        context.command().args(["util", "yaml-to-toml"]),
        @"
    success: true
    exit_code: 0
    ----- stdout -----
    Converted `.pre-commit-config.yaml` → `prek.toml`

    ----- stderr -----
    "
    );

    context
        .work_dir()
        .child(PREK_TOML)
        .assert(predicates::path::exists());

    Ok(())
}

#[test]
fn yaml_to_toml_discovers_pre_commit_config_yml() -> anyhow::Result<()> {
    let context = TestContext::new();

    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YML)
        .write_str(YAML_CONFIG)?;

    cmd_snapshot!(
        context.filters(),
        context.command().args(["util", "yaml-to-toml"]),
        @"
    success: true
    exit_code: 0
    ----- stdout -----
    Converted `.pre-commit-config.yml` → `prek.toml`

    ----- stderr -----
    "
    );

    context
        .work_dir()
        .child(PREK_TOML)
        .assert(predicates::path::exists());

    Ok(())
}

#[test]
fn yaml_to_toml_prefers_yaml_over_yml() -> anyhow::Result<()> {
    let context = TestContext::new();

    // Write different content to each file so we can verify which was used.
    let yaml_only = indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: trailing-whitespace
    "};
    let yml_only = indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
    "};

    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YAML)
        .write_str(yaml_only)?;
    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YML)
        .write_str(yml_only)?;

    cmd_snapshot!(
        context.filters(),
        context.command().args(["util", "yaml-to-toml"]),
        @"
    success: true
    exit_code: 0
    ----- stdout -----
    Converted `.pre-commit-config.yaml` → `prek.toml`

    ----- stderr -----
    "
    );

    // The .yaml file contains trailing-whitespace, the .yml contains end-of-file-fixer.
    let output = context.read(PREK_TOML);
    assert!(
        output.contains("trailing-whitespace"),
        "Expected .yaml to be preferred over .yml"
    );

    Ok(())
}

#[test]
fn yaml_to_toml_error_when_no_config_found() {
    let context = TestContext::new();

    cmd_snapshot!(
        context.filters(),
        context.command().args(["util", "yaml-to-toml"]),
        @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: No `.pre-commit-config.yaml` or `.pre-commit-config.yml` found in the current directory

    hint: Provide a path explicitly: prek util yaml-to-toml <CONFIG>
    "#
    );
}
