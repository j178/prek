use anyhow::Result;
use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

/// Test `language_version` parsing and installation for Rust hooks.
#[test]
fn language_version() -> Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other rust versions installed locally.
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust-system
                name: rust-system
                language: rust
                entry: rustc --version
                language_version: system
                pass_filenames: false
                always_run: true
              - id: rust-1.70 # should auto install 1.70.X
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
              - id: rust-1.70 # run again to ensure reusing the installed version
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    let rust_dir = context.home_dir().child("tools/rustup/toolchains");
    rust_dir.assert(predicates::path::missing());

    let filters = [
        (r"rustc (1\.70)\.\d{1,2} .+", "rustc $1.X"), // Keep 1.70.X format
        (r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X"), // Others become 1.X.X
    ]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-system..............................................................Passed
    - hook id: rust-system
    - duration: [TIME]

      rustc 1.X.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]

      rustc 1.70.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]

      rustc 1.70.X

    ----- stderr -----
    "#);

    // Ensure that only Rust 1.70.X is installed.
    let installed_versions = rust_dir
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
        "Expected only one Rust version to be installed, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions.iter().any(|v| v.starts_with("1.70")),
        "Expected Rust 1.70.X to be installed, but found: {installed_versions:?}"
    );

    Ok(())
}

/// Test `rustup` installer.
#[test]
fn rustup_installer() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rustup-test
                name: rustup-test
                language: rust
                entry: rustc --version
   "});
    context.git_add(".");
    let filters = [(r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X")]
        .into_iter()
        .chain(context.filters())
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v").env(EnvVars::PREK_INTERNAL__RUSTUP_BINARY_NAME, "non-exist-rustup"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rustup-test..............................................................Passed
    - hook id: rustup-test
    - duration: [TIME]

      rustc 1.X.X

    ----- stderr -----
    "#);
}

/// Test that `additional_dependencies` with cli: prefix are installed correctly.
#[test]
fn additional_dependencies_cli() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: rust-cli
                name: rust-cli
                language: rust
                entry: prek-rust-echo Hello, Prek!
                additional_dependencies: ["cli:prek-rust-echo"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-cli.................................................................Passed
    - hook id: rust-cli
    - duration: [TIME]

      Hello, Prek!

    ----- stderr -----
    ");
}

/// Test that remote Rust hooks are installed and run correctly.
#[test]
fn remote_hooks() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-test-repos/rust-hooks
            rev: v1.0.0
            hooks:
              - id: hello-world
                verbose: true
                pass_filenames: false
                always_run: true
                args: ["Hello World"]
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World..............................................................Passed
    - hook id: hello-world
    - duration: [TIME]

      Hello World

    ----- stderr -----
    ");
}

/// Test that library dependencies (non-cli: prefix) work correctly on remote hooks.
/// This verifies that the shared repo is not modified when adding dependencies.
#[test]
fn remote_hooks_with_lib_deps() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-test-repos/rust-hooks
            rev: v1.0.0
            hooks:
              - id: hello-world-lib-deps
                additional_dependencies: ["itoa:1"]
                verbose: true
                pass_filenames: false
                always_run: true
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World Lib Deps.....................................................Passed
    - hook id: hello-world-lib-deps
    - duration: [TIME]

      42

    ----- stderr -----
    ");
}
