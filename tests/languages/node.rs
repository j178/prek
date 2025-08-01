use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;

use crate::common::{TestContext, cmd_snapshot};

// GitHub Actions ubuntu-latest (24.04) has node 20.19.4 installed at the moment.
// And we use `setup-node` action to install node 19.9.0
#[test]
fn language_version() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: '19'
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: node19
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: '18.20.8' # will auto download
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: node18.20.8
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: '<20'
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: 'lts/hydrogen'
                always_run: true
    "});
    context.git_add(".");

    context
        .home_dir()
        .child("tools")
        .child("node")
        .assert(predicates::path::missing());

    cmd_snapshot!(context.filters(), context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v19.9.0
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v19.9.0
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v18.20.8
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v18.20.8
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v19.9.0
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v18.20.8

    ----- stderr -----
    "#);

    assert_eq!(
        context
            .home_dir()
            .join("tools")
            .join("node")
            .read_dir()?
            .flatten()
            .filter(|d| !d.file_name().to_string_lossy().starts_with('.'))
            .map(|d| d.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        vec!["18.20.8-Hydrogen"],
    );

    Ok(())
}

/// Test that `additional_dependencies` are installed correctly.
#[test]
fn additional_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: node
                name: node
                language: node
                entry: cowsay Hello World!
                additional_dependencies: ["cowsay"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      ______________
      < Hello World! >
       --------------
              \   ^__^
               \  (oo)/_______
                  (__)\       )\/\
                      ||----w |
                      ||     ||

    ----- stderr -----
    "###);
}
