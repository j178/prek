use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild};
use prek_consts::PREK_TOML;

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
        @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Written to `prek.toml`

    ----- stderr -----
    "#
    );

    insta::assert_snapshot!(context.read(PREK_TOML), @r#"
    # Configuration file for `prek`, a git hook framework written in Rust.
    # See https://prek.j178.dev for more information.
    #:schema https://www.schemastore.org/prek.json
    #:tombi toml-version = "v1.1.0"

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
    Written to `prek.toml`

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
      caused by: error: line 1 column 8: unexpected event: expected sequence start at line 1, column 8
     --> <input>:1:8
      |
    1 | repos: 123
      |        ^ unexpected event: expected sequence start at line 1, column 8
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
