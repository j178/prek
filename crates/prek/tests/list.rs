use crate::common::{TestContext, cmd_snapshot};
use indoc::indoc;

mod common;

#[test]
fn list_basic() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-yaml
                name: Check YAML
                entry: check-yaml
                language: system
                types: [yaml]
              - id: check-json
                name: Check JSON
                entry: check-json
                language: system
                types: [json]
                description: Validate JSON files
    "});

    cmd_snapshot!(context.filters(), context.list(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml
    .:check-json

    ----- stderr -----
    ");
}

#[test]
fn list_verbose() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-yaml
                name: Check YAML
                entry: check-yaml
                language: system
                types: [yaml]
              - id: check-json
                name: Check JSON
                entry: check-json
                language: system
                types: [json]
                description: Validate JSON files
                fail_fast: true
                verbose: true
    "});

    cmd_snapshot!(context.filters(), context.list().arg("--verbose"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml
      ID: check-yaml
      Name: Check YAML
      Language: system
      Stages: all

    .:check-json
      ID: check-json
      Name: Check JSON
      Description: Validate JSON files
      Language: system
      Stages: all


    ----- stderr -----
    ");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: custom-formatter
                name: Custom Code Formatter
                entry: ./format.sh
                language: script
                description: Custom formatting tool with specific requirements
                files: \.(py|rs|js)$
                exclude: vendor/
                types: [text]
                types_or: [python, rust, javascript]
                exclude_types: [binary]
                args: [--check, --diff]
                always_run: true
                fail_fast: true
                pass_filenames: false
                require_serial: true
                verbose: true
                stages: [pre-commit, pre-push]
                alias: fmt
    "});

    cmd_snapshot!(context.filters(), context.list().arg("--verbose"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:custom-formatter
      ID: custom-formatter
      Alias: fmt
      Name: Custom Code Formatter
      Description: Custom formatting tool with specific requirements
      Language: script
      Stages: pre-commit, pre-push


    ----- stderr -----
    ");
}

#[test]
fn list_with_hook_ids_filter() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-yaml
                name: Check YAML
                entry: check-yaml
                language: system
                types: [yaml]
              - id: check-json
                name: Check JSON
                entry: check-json
                language: system
                types: [json]
              - id: check-toml
                name: Check TOML
                entry: check-toml
                language: system
                types: [toml]
    "});

    // Test filtering by specific hook ID
    cmd_snapshot!(context.filters(), context.list().arg("check-yaml"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml

    ----- stderr -----
    ");

    // Test filtering by multiple hook IDs
    cmd_snapshot!(context.filters(), context.list().arg("check-yaml").arg("check-json"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml
    .:check-json

    ----- stderr -----
    ");
}

#[test]
fn list_with_language_filter() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-yaml
                name: Check YAML
                entry: check-yaml
                language: system
                types: [yaml]
              - id: format-python
                name: Format Python
                entry: black
                language: python
                types: [python]
              - id: lint-rust
                name: Lint Rust
                entry: clippy
                language: rust
                types: [rust]
    "});

    // Test filtering by language
    cmd_snapshot!(context.filters(), context.list().arg("--language").arg("system"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg("--language").arg("python"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:format-python

    ----- stderr -----
    ");
}

#[test]
fn list_with_stage_filter() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-yaml
                name: Check YAML
                entry: check-yaml
                language: system
                types: [yaml]
              - id: check-json
                name: Check JSON
                entry: check-json
                language: system
                types: [json]
                stages: [pre-push]
              - id: check-toml
                name: Check TOML
                entry: check-toml
                language: system
                types: [toml]
                stages: [pre-commit, pre-push]
    "});

    // Test filtering by stage
    cmd_snapshot!(context.filters(), context.list().arg("--hook-stage").arg("pre-commit"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml
    .:check-toml

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg("--hook-stage").arg("pre-push"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml
    .:check-json
    .:check-toml

    ----- stderr -----
    ");
}

#[test]
fn list_with_aliases() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-yaml
                name: Check YAML
                entry: check-yaml
                language: system
                types: [yaml]
                alias: yaml-check
              - id: check-json
                name: Check JSON
                entry: check-json
                language: system
                types: [json]
    "});

    // Test that aliases are recognized
    cmd_snapshot!(context.filters(), context.list().arg("yaml-check"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml

    ----- stderr -----
    ");

    // Test verbose shows alias information
    cmd_snapshot!(context.filters(), context.list().arg("--verbose").arg("check-yaml"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:check-yaml
      ID: check-yaml
      Alias: yaml-check
      Name: Check YAML
      Language: system
      Stages: all


    ----- stderr -----
    ");
}

#[test]
fn list_empty_config() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config("repos: []");

    cmd_snapshot!(context.filters(), context.list(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.list().arg("--verbose"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    "#);
}

#[test]
fn list_no_config_file() {
    let context = TestContext::new();
    context.init_project();

    // No config file exists
    cmd_snapshot!(context.filters(), context.list(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: No `prek.toml` or `.pre-commit-config.yaml` found in the current directory or parent directories.

    hint: If you just added one, rerun your command with the `--refresh` flag to rescan the workspace.
    ");
}

#[test]
fn list_json_output() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-yaml
                name: Check YAML
                entry: check-yaml
                language: system
                types: [yaml]
                alias: yaml-check
              - id: check-json
                name: Check JSON
                entry: check-json
                language: system
                types: [json]
                description: Validate JSON files
    "});

    // Test JSON output for all hooks
    cmd_snapshot!(context.filters(), context.list().arg("--output-format=json"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [
      {
        "id": "check-yaml",
        "full_id": ".:check-yaml",
        "name": "Check YAML",
        "alias": "yaml-check",
        "language": "system",
        "description": null,
        "stages": [
          "manual",
          "commit-msg",
          "post-checkout",
          "post-commit",
          "post-merge",
          "post-rewrite",
          "pre-commit",
          "pre-merge-commit",
          "pre-push",
          "pre-rebase",
          "prepare-commit-msg"
        ]
      },
      {
        "id": "check-json",
        "full_id": ".:check-json",
        "name": "Check JSON",
        "alias": "",
        "language": "system",
        "description": "Validate JSON files",
        "stages": [
          "manual",
          "commit-msg",
          "post-checkout",
          "post-commit",
          "post-merge",
          "post-rewrite",
          "pre-commit",
          "pre-merge-commit",
          "pre-push",
          "pre-rebase",
          "prepare-commit-msg"
        ]
      }
    ]

    ----- stderr -----
    "#);

    // Test filtered JSON output
    cmd_snapshot!(context.filters(), context.list().arg("check-json").arg("--output-format=json"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [
      {
        "id": "check-json",
        "full_id": ".:check-json",
        "name": "Check JSON",
        "alias": "",
        "language": "system",
        "description": "Validate JSON files",
        "stages": [
          "manual",
          "commit-msg",
          "post-checkout",
          "post-commit",
          "post-merge",
          "post-rewrite",
          "pre-commit",
          "pre-merge-commit",
          "pre-push",
          "pre-rebase",
          "prepare-commit-msg"
        ]
      }
    ]

    ----- stderr -----
    "#);
}

#[test]
fn workspace_list() -> anyhow::Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();

    let config = indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: show-cwd
          name: Show CWD
          language: python
          entry: python -c 'import sys, os; print(os.getcwd()); print(sys.argv[1:])'
          verbose: true
    "};

    context.setup_workspace(
        &[
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    )?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.list(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    nested/project4:show-cwd
    project3/project5:show-cwd
    project2:show-cwd
    project3:show-cwd
    .:show-cwd

    ----- stderr -----
    ");

    let mut filters = context.filters();
    filters.push((r"\\/", "/")); // Normalize Windows path separators in JSON output
    cmd_snapshot!(filters, context.list().arg("--output-format=json"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [
      {
        "id": "show-cwd",
        "full_id": "nested/project4:show-cwd",
        "name": "Show CWD",
        "alias": "",
        "language": "python",
        "description": null,
        "stages": [
          "manual",
          "commit-msg",
          "post-checkout",
          "post-commit",
          "post-merge",
          "post-rewrite",
          "pre-commit",
          "pre-merge-commit",
          "pre-push",
          "pre-rebase",
          "prepare-commit-msg"
        ]
      },
      {
        "id": "show-cwd",
        "full_id": "project3/project5:show-cwd",
        "name": "Show CWD",
        "alias": "",
        "language": "python",
        "description": null,
        "stages": [
          "manual",
          "commit-msg",
          "post-checkout",
          "post-commit",
          "post-merge",
          "post-rewrite",
          "pre-commit",
          "pre-merge-commit",
          "pre-push",
          "pre-rebase",
          "prepare-commit-msg"
        ]
      },
      {
        "id": "show-cwd",
        "full_id": "project2:show-cwd",
        "name": "Show CWD",
        "alias": "",
        "language": "python",
        "description": null,
        "stages": [
          "manual",
          "commit-msg",
          "post-checkout",
          "post-commit",
          "post-merge",
          "post-rewrite",
          "pre-commit",
          "pre-merge-commit",
          "pre-push",
          "pre-rebase",
          "prepare-commit-msg"
        ]
      },
      {
        "id": "show-cwd",
        "full_id": "project3:show-cwd",
        "name": "Show CWD",
        "alias": "",
        "language": "python",
        "description": null,
        "stages": [
          "manual",
          "commit-msg",
          "post-checkout",
          "post-commit",
          "post-merge",
          "post-rewrite",
          "pre-commit",
          "pre-merge-commit",
          "pre-push",
          "pre-rebase",
          "prepare-commit-msg"
        ]
      },
      {
        "id": "show-cwd",
        "full_id": ".:show-cwd",
        "name": "Show CWD",
        "alias": "",
        "language": "python",
        "description": null,
        "stages": [
          "manual",
          "commit-msg",
          "post-checkout",
          "post-commit",
          "post-merge",
          "post-rewrite",
          "pre-commit",
          "pre-merge-commit",
          "pre-push",
          "pre-rebase",
          "prepare-commit-msg"
        ]
      }
    ]

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.list().current_dir(cwd.join("project3")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    project5:show-cwd
    .:show-cwd

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().current_dir(cwd.join("project3")).arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    project5:show-cwd
      ID: show-cwd
      Name: Show CWD
      Language: python
      Stages: all

    .:show-cwd
      ID: show-cwd
      Name: Show CWD
      Language: python
      Stages: all


    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn list_with_selectors() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let config = indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: show-cwd
          name: Show CWD
          language: python
          entry: python -c 'import sys, os; print(os.getcwd()); print(sys.argv[1:])'
          verbose: true
    "};

    context.setup_workspace(
        &[
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    )?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.list().arg("project2/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    project2:show-cwd

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg("--skip").arg("project2/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    nested/project4:show-cwd
    project3/project5:show-cwd
    project3:show-cwd
    .:show-cwd

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg("--skip").arg("nested/").arg("--skip").arg("project3/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    project2:show-cwd
    .:show-cwd

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg("show-cwd"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    nested/project4:show-cwd
    project3/project5:show-cwd
    project2:show-cwd
    project3:show-cwd
    .:show-cwd

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg("project2:show-cwd"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    project2:show-cwd

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg(".:show-cwd"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:show-cwd

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg("--skip").arg("show-cwd"), @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.list().arg("--skip").arg("project2:show-cwd").arg("--skip").arg("nested:show-cwd"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    nested/project4:show-cwd
    project3/project5:show-cwd
    project3:show-cwd
    .:show-cwd

    ----- stderr -----
    warning: selector `--skip=nested:show-cwd` did not match any hooks
    ");

    cmd_snapshot!(context.filters(), context.list().arg("--skip").arg("non-exist"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    nested/project4:show-cwd
    project3/project5:show-cwd
    project2:show-cwd
    project3:show-cwd
    .:show-cwd

    ----- stderr -----
    warning: selector `--skip=non-exist` did not match any hooks
    ");

    cmd_snapshot!(context.filters(), context.list().arg("--skip").arg("../"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Invalid selector: `../`
      caused by: Invalid project path: `../`
      caused by: path is outside the workspace root
    ");

    cmd_snapshot!(context.filters(), context.list().current_dir(context.work_dir().join("project2")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .:show-cwd

    ----- stderr -----
    ");

    Ok(())
}
