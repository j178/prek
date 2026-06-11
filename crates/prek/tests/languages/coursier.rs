use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;

use crate::common::{TestContext, cmd_snapshot};

#[test]
fn additional_dependencies() {
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
fn pre_commit_channel() -> anyhow::Result<()> {
    let hook_repo = TestContext::new();
    hook_repo.init_project();

    hook_repo
        .work_dir()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
            - id: echo-java
              name: echo-java
              language: coursier
              entry: echo-java Hello World from coursier
        "})?;

    let channel_dir = hook_repo.work_dir().child(".pre-commit-channel");
    channel_dir.create_dir_all()?;
    channel_dir
        .child("echo-java.json")
        .write_str(indoc::indoc! {r#"
            {
              "repositories": ["central"],
              "dependencies": ["io.get-coursier:echo:latest.stable"]
            }
        "#})?;

    hook_repo.git_add(".");
    hook_repo.git_commit("Add coursier hook");
    hook_repo.git_tag("v1.0.0");

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: echo-java
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo.work_dir().display()});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-java................................................................Passed
    - hook id: echo-java
    - duration: [TIME]

      Hello World from coursier

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn local_pre_commit_channel_is_ignored() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let channel_dir = context.work_dir().child(".pre-commit-channel");
    channel_dir.create_dir_all()?;
    channel_dir.child("scalafmt.json").write_str("{}")?;

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
      caused by: expected `.pre-commit-channel` directory or `additional_dependencies`
    ");

    Ok(())
}
