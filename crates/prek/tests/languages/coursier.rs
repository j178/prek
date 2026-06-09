use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

#[test]
fn additional_dependencies() {
    if !EnvVars::is_set(EnvVars::CI) {
        return;
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: scalafmt
                name: scalafmt
                language: coursier
                entry: scalafmt --version
                additional_dependencies: ["scalafmt:3.6.1"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    scalafmt.................................................................Passed
    - hook id: scalafmt
    - duration: [TIME]

      scalafmt 3.6.1

    ----- stderr -----
    ");
}

#[test]
fn missing_channel_and_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: scalafmt
                name: scalafmt
                language: coursier
                entry: scalafmt --version
                always_run: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `scalafmt`
      caused by: expected .pre-commit-channel dir or additional_dependencies
    ");
}
