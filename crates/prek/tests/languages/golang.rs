use std::process::Command;

use assert_fs::assert::PathAssert;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::env_vars::EnvVars;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_HOOKS_YAML};

use crate::common::{TestContext, cmd_snapshot};

/// Test `language_version` parsing and installation for golang hooks.
/// We use `setup-go` action to install go 1.24 in CI, so go 1.23 will be auto downloaded.
#[test]
fn language_version() -> anyhow::Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        // Skip when not running in CI, as we may have other go versions installed locally.
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: '1.24'
                pass_filenames: false
                always_run: true
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: go1.24
                always_run: true
                pass_filenames: false
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: '1.23' # will auto download
                always_run: true
                pass_filenames: false
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: go1.23
                always_run: true
                pass_filenames: false
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: go1.23
                always_run: true
                pass_filenames: false
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: '<1.25'
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    let go_dir = context.home_dir().child("tools").child("go");
    go_dir.assert(predicates::path::missing());

    let filters = [(
        r"go version (go1\.\d{1,2})\.\d{1,2} ([\w]+/[\w]+)",
        "go version $1.X [OS]/[ARCH]",
    )]
    .into_iter()
    .chain(context.filters())
    .collect::<Vec<_>>();

    cmd_snapshot!(filters, context.run().arg("-v"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]

      go version go1.24.X [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]

      go version go1.24.X [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]

      go version go1.23.X [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]

      go version go1.23.X [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]

      go version go1.23.X [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]

      go version go1.24.X [OS]/[ARCH]

    ----- stderr -----
    "#);

    // Check that only go 1.23 is installed.
    let installed_versions = go_dir
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
        "Expected only one Go version to be installed, but found: {installed_versions:?}"
    );
    assert!(
        installed_versions.iter().any(|v| v.contains("1.23")),
        "Expected Go 1.23 to be installed, but found: {installed_versions:?}"
    );

    Ok(())
}

/// Test a remote go hook.
#[test]
fn remote_hook() {
    let context = TestContext::new();
    context.init_project();

    // Run hooks with system found go.
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/golang-hooks
            rev: v1.0
            hooks:
              - id: echo
                verbose: true
        "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]

      .pre-commit-config.yaml

    ----- stderr -----
    ");

    // Test that `additional_dependencies` are installed correctly.
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: golang
                name: golang
                language: golang
                entry: gofumpt -h
                additional_dependencies: ["mvdan.cc/gofumpt@v0.8.0"]
                always_run: true
                verbose: true
                language_version: '1.23.11' # will auto download
                pass_filenames: false
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]

      usage: gofumpt [flags] [path ...]
      	-version  show version and exit

      	-d        display diffs instead of rewriting files
      	-e        report all errors (not just the first 10 on different lines)
      	-l        list files whose formatting differs from gofumpt's
      	-w        write result to (source) file instead of stdout
      	-extra    enable extra rules which should be vetted by a human

      	-lang       str    target Go version in the form "go1.X" (default from go.mod)
      	-modpath    str    Go module path containing the source file (default from go.mod)

    ----- stderr -----
    "#);

    // Run hooks with newly downloaded go.
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/golang-hooks
            rev: v1.0
            hooks:
              - id: echo
                verbose: true
                language_version: '1.23.11' # will auto download
        "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]

      .pre-commit-config.yaml

    ----- stderr -----
    ");
}

/// Fix <https://github.com/j178/prek/issues/901>
#[test]
fn local_additional_deps() -> anyhow::Result<()> {
    let go_hook = TestContext::new();
    go_hook.init_project();
    go_hook.configure_git_author();
    go_hook.disable_auto_crlf();

    // Create a local go hook with additional_dependencies.
    go_hook
        .work_dir()
        .child("go.mod")
        .write_str(indoc::indoc! {r"
        module example.com/go-hook
    "})?;
    go_hook
        .work_dir()
        .child("main.go")
        .write_str(indoc::indoc! {r#"
        package main

        func main() {
            println("Hello, World!")
        }
    "#})?;
    go_hook.work_dir().child("cmd").create_dir_all()?;
    go_hook
        .work_dir()
        .child("cmd/main.go")
        .write_str(indoc::indoc! {r#"
        package main

        func main() {
            println("Hello, Utility!")
        }
    "#})?;
    go_hook
        .work_dir()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
        - id: go-hook
          name: go-hook
          entry: cmd
          language: golang
          additional_dependencies: [ ./cmd ]
    "})?;
    go_hook.git_add(".");
    go_hook.git_commit("Initial commit");
    Command::new("git")
        .args(["tag", "v1.0", "-m", "v1.0"])
        .current_dir(go_hook.work_dir())
        .output()?;

    let context = TestContext::new();
    context.init_project();
    let work_dir = context.work_dir();

    let hook_url = go_hook.work_dir().to_str().unwrap();
    work_dir
        .child(PRE_COMMIT_CONFIG_YAML)
        .write_str(&indoc::formatdoc! {r"
        repos:
          - repo: {hook_url}
            rev: v1.0
            hooks:
              - id: go-hook
                verbose: true
   ", hook_url = hook_url})?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    go-hook..................................................................Passed
    - hook id: go-hook
    - duration: [TIME]

      Hello, Utility!

    ----- stderr -----
    ");

    Ok(())
}
