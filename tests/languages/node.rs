use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

/// Test `language_version` parsing and auto downloading works correctly.
/// We use `setup-node` action to install node 20 in CI, so node 19 should be downloaded by prek.
#[test]
fn language_version() -> anyhow::Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other node versions installed locally.
        return Ok(());
    }

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
                language_version: '20'
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: node20
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: '19' # will auto download
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
                language_version: '<20'
                always_run: true
              - id: node
                name: node
                language: node
                entry: node -p 'process.version'
                language_version: 'lts/iron' # node 20
                always_run: true
    "});
    context.git_add(".");

    let node_dir = context.home_dir().child("tools").child("node");
    node_dir.assert(predicates::path::missing());

    let filters = context
        .filters()
        .into_iter()
        .chain([(r"v(\d+)\.\d+.\d+", "v$1.X.X")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v20.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v20.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v19.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v19.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v19.X.X
    node.....................................................................Passed
    - hook id: node
    - duration: [TIME]
      v20.X.X

    ----- stderr -----
    "#);

    // Check that only node 19 is installed.
    let installed_versions = node_dir
        .read_dir()?
        .flatten()
        .filter_map(|d| {
            let filename = d.file_name().to_string_lossy().to_string();
            if filename.starts_with('.') {
                None
            } else {
                Some(filename)
            }
        })
        .collect::<Vec<_>>();

    assert_eq!(
        installed_versions.len(),
        1,
        "Expected only one node version to be installed, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions.iter().any(|v| v.starts_with("19")),
        "Expected node v19 to be installed, but found: {installed_versions:?}"
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

    // Run again to check `health_check` works correctly.
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
