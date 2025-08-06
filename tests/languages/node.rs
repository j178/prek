use std::path::PathBuf;

use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

// We use `setup-node` action to install node 19.9.0 in CI, so 18.20.8 should be downloaded by prefligit.
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

fn remove_node_from_path() -> anyhow::Result<PathBuf> {
    let node_dirs: std::collections::HashSet<_> = which::which_all("node")
        .unwrap_or_else(|_| Vec::new().into_iter())
        .filter_map(|path| path.parent())
        .collect();

    let current_path = std::env::var("PATH").unwrap_or_default();

    let new_path_entries: Vec<_> = std::env::split_paths(&current_path)
        .filter(|path| !node_dirs.contains(path.as_path()))
        .collect();

    Ok(std::env::join_paths(new_path_entries)?)
}

/// Test `https://github.com/thlorenz/doctoc` works correctly with prefligit.
/// Previously, prefligit did not install its dependencies correctly.
#[test]
fn doctoc() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/thlorenz/doctoc
            rev: v2.2.0
            hooks:
              - id: doctoc
                name: Add TOC for Markdown
    "});
    context
        .work_dir()
        .child("README.md")
        .write_str("# Hello World\n\nThis is a test file.\n\n## Subsection\n\nMore content here.\n")
        .unwrap();
    context.git_add(".");

    let path = remove_node_from_path().unwrap();

    // Set PATH to . to mask the system installed node,
    // ensure that `npm` runs correctly.
    cmd_snapshot!(context.filters(), context.run().env("PATH", path), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Add TOC for Markdown.....................................................Failed
    - hook id: doctoc
    - files were modified by this hook
      DocToccing single file "README.md" for github.com.

      ==================

      "README.md" will be updated

      Everything is OK.

    ----- stderr -----
    "#);
}
