use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;
use constants::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

/// Test `language_version` parsing and installation for Rust hooks.
#[test]
fn language_version() {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other rust versions installed locally.
        return;
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rust-stable
                name: rust-stable
                language: rust
                entry: rustc --version
                language_version: 'stable'
                pass_filenames: false
                always_run: true
              - id: rust-1.70
                name: rust-1.70
                language: rust
                entry: rustc --version
                language_version: '1.70'
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    let rust_dir = context.home_dir().child("tools").child("rust");
    rust_dir.assert(predicates::path::missing());

    let filters = [
        (r"rustc (1\.70)\.\d{1,2}", "rustc $1.X"), // Keep 1.70.X format
        (r"rustc 1\.\d{1,3}\.\d{1,2}", "rustc 1.X"), // Others become 1.X
        (r"\([a-f0-9]+ \d{4}-\d{2}-\d{2}\)", ""),  // Remove commit hash and date
        (r"  info: .*\n", ""),                     // Remove rustup info lines
        (r" +\n", "\n"),                           // Remove trailing whitespace from lines
    ]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-stable..............................................................Passed
    - hook id: rust-stable
    - duration: [TIME]
      rustc 1.X
    rust-1.70................................................................Passed
    - hook id: rust-1.70
    - duration: [TIME]
      rustc 1.70.X

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

/// Test that system Rust can be used.
#[test]
fn system_rust() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: system
                name: system
                language: rust
                entry: rustc --version
                language_version: system
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    system...................................................................Passed

    ----- stderr -----
    ");
}
