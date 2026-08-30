use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn additional_dependencies() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
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
    "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @"
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
fn pre_commit_channel() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "coursier-hook",
            indoc::indoc! {r"
            - id: echo-java
              name: echo-java
              language: coursier
              entry: echo-java Hello World from coursier
        "},
        )
        .with_file(
            ".pre-commit-channel/echo-java.json",
            indoc::indoc! {r#"
            {
              "repositories": ["central"],
              "dependencies": ["io.get-coursier:echo:latest.stable"]
            }
        "#},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: echo-java
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo});

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-java................................................................Passed
    - hook id: echo-java
    - duration: [TIME]

      Hello World from coursier

    ----- stderr -----
    ");
}

#[test]
fn local_pre_commit_channel_is_ignored() {
    let context = TestEnv::new()
        .with_file(".pre-commit-channel/scalafmt.json", "{}")
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: scalafmt
                name: scalafmt
                language: coursier
                entry: scalafmt --version
                always_run: true
                pass_filenames: false
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `scalafmt`
      caused by: expected `.pre-commit-channel` directory or `additional_dependencies`
    ");
}
