use assert_fs::assert::PathAssert;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML, PREK_TOML};

use crate::common::{TestEnv, cmd_snapshot};

mod common;

const YAML_CONFIG: &str = r#"
fail_fast: true
default_install_hook_types: [pre-push]
priorities:
  checks: 1
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
        priority: checks
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
fn yaml_to_toml_writes_default_output() {
    let context = TestEnv::new().with_file("config.yaml", YAML_CONFIG);

    cmd_snapshot!(context,
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
    priorities = { checks = 1 }
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
        priority = "checks",
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
}

#[test]
fn yaml_to_toml_force_overwrite() {
    let context = TestEnv::new()
        .with_file("config.yaml", YAML_CONFIG)
        .with_file(PREK_TOML, "existing");

    cmd_snapshot!(context,
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

    cmd_snapshot!(context,
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
}

#[test]
fn yaml_to_toml_rejects_invalid_config() {
    let context = TestEnv::new().with_file("config.yaml", "repos: 123");

    cmd_snapshot!(context,
      context
        .command()
        .args(["util", "yaml-to-toml", "config.yaml"]),
      @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `config.yaml`
      caused by: error: line 1 column 8: expected sequence start
     --> <input>:1:8
      |
    1 | repos: 123
      |        ^ expected sequence start
    "#
    );
}

#[test]
fn yaml_to_toml_same_output() {
    let context = TestEnv::new().with_file("config.yaml", YAML_CONFIG);

    cmd_snapshot!(context,
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

    context.child(PREK_TOML).assert(predicates::path::missing());
}

#[test]
fn yaml_to_toml_discovers_pre_commit_config_yaml() {
    let context = TestEnv::new().with_file(PRE_COMMIT_CONFIG_YAML, YAML_CONFIG);

    cmd_snapshot!(context,
        context.command().args(["util", "yaml-to-toml"]),
        @"
    success: true
    exit_code: 0
    ----- stdout -----
    Converted `.pre-commit-config.yaml` → `prek.toml`

    ----- stderr -----
    "
    );

    context.child(PREK_TOML).assert(predicates::path::exists());
}

#[test]
fn yaml_to_toml_discovers_pre_commit_config_yml() {
    let context = TestEnv::new().with_file(PRE_COMMIT_CONFIG_YML, YAML_CONFIG);

    cmd_snapshot!(context,
        context.command().args(["util", "yaml-to-toml"]),
        @"
    success: true
    exit_code: 0
    ----- stdout -----
    Converted `.pre-commit-config.yml` → `prek.toml`

    ----- stderr -----
    "
    );

    context.child(PREK_TOML).assert(predicates::path::exists());
}

#[test]
fn yaml_to_toml_prefers_yaml_over_yml() {
    let context = TestEnv::new();

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

    let context = context
        .with_file(PRE_COMMIT_CONFIG_YAML, yaml_only)
        .with_file(PRE_COMMIT_CONFIG_YML, yml_only);

    cmd_snapshot!(context,
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
}

#[test]
fn yaml_to_toml_error_when_no_config_found() {
    let context = TestEnv::new();

    cmd_snapshot!(context,
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
