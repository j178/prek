use anyhow::Result;
use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use crate::common::{TestEnv, cmd_snapshot};

/// Test `language_version` parsing and installation for Rust hooks.
#[test]
fn language_version() -> Result<()> {
    if !EnvVars.is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other rust versions installed locally.
        return Ok(());
    }

    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
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
    context.git().add_all();

    let rust_dir = context.home_dir().child("tools/rustup/toolchains");
    rust_dir.assert(predicates::path::missing());

    let context = context.with_filters([
        (r"rustc (1\.70)\.\d{1,2} .+", "rustc $1.X"), // Keep 1.70.X format
        (r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X"), // Others become 1.X.X
    ]);

    cmd_snapshot!(context, context.run().arg("-v"), @r#"
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
            let filename = d.file_name().to_string_lossy().into_owned();
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
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rustup-test
                name: rustup-test
                language: rust
                entry: rustc --version
   "});
    context.git().add_all();
    let context = context.with_filter(r"rustc 1\.\d{1,3}\.\d{1,2} .+", "rustc 1.X.X");

    cmd_snapshot!(context, context.run().arg("-v").env(EnvVars::PREK_INTERNAL__RUSTUP_BINARY_NAME, "non-exist-rustup"), @r#"
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
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
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
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-ci/rust-hooks
            rev: v1.0.0
            hooks:
              - id: hello-world
                verbose: true
                pass_filenames: false
                always_run: true
                args: ["Hello World"]
    "#});
    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World..............................................................Passed
    - hook id: hello-world
    - description: echoes its arguments
    - duration: [TIME]

      Hello World

    ----- stderr -----
    ");
}

/// Test that remote Rust hooks from non-workspace repos are installed and run correctly.
#[test]
fn remote_hook_non_workspace() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-ci/rust-hooks-non-workspace
            rev: v1.0.0
            hooks:
              - id: hello-world
                verbose: true
                pass_filenames: false
                always_run: true
    "});
    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello-world..............................................................Passed
    - hook id: hello-world
    - description: A simple hello world hook
    - duration: [TIME]

      Hello, Prek!

    ----- stderr -----
    ");
}

/// Test that library dependencies (non-cli: prefix) work correctly on remote hooks.
/// This verifies that the shared repo is not modified when adding dependencies.
#[test]
fn remote_hooks_with_lib_deps() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-ci/rust-hooks
            rev: v1.0.0
            hooks:
              - id: hello-world-lib-deps
                additional_dependencies: ["itoa:1"]
                verbose: true
                pass_filenames: false
                always_run: true
    "#});
    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World Lib Deps.....................................................Passed
    - hook id: hello-world-lib-deps
    - description: requires a library dependency to be added
    - duration: [TIME]

      42

    ----- stderr -----
    ");
}
