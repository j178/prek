use assert_fs::assert::PathAssert;
use assert_fs::fixture::PathChild;

use crate::common::{TestContext, cmd_snapshot};

// We use `setup-go` action to install go1.24.5 in CI, so 1.23.11 should be downloaded by prefligit.
#[test]
fn language_version() -> anyhow::Result<()> {
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
                language_version: '1.24.5'
                pass_filenames: false
                always_run: true
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: go1.24.5
                always_run: true
                pass_filenames: false
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: '1.23.11' # will auto download
                always_run: true
                pass_filenames: false
              - id: golang
                name: golang
                language: golang
                entry: go version
                language_version: go1.23.11
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

    context
        .home_dir()
        .child("tools")
        .child("go")
        .assert(predicates::path::missing());

    let filters = [(
        r"(go version go1\.\d{1,2}\.\d{1,2}) ([\w]+/[\w]+)",
        "$1 [OS]/[ARCH]",
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
      go version go1.24.5 [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]
      go version go1.24.5 [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]
      go version go1.23.11 [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]
      go version go1.23.11 [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]
      go version go1.23.11 [OS]/[ARCH]
    golang...................................................................Passed
    - hook id: golang
    - duration: [TIME]
      go version go1.24.5 [OS]/[ARCH]

    ----- stderr -----
    "#);

    assert_eq!(
        context
            .home_dir()
            .join("tools")
            .join("go")
            .read_dir()?
            .flatten()
            .filter(|d| !d.file_name().to_string_lossy().starts_with('.'))
            .map(|d| d.file_name().to_string_lossy().to_string())
            .collect::<Vec<_>>(),
        vec!["1.23.11"],
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
}

/// Test a remote go hook.
#[test]
fn remote_hook() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prefligit-test-repos/golang-hooks
            rev: main
            hooks:
              - id: echo
                verbose: true
                language_version: '1.23.11' # will auto download
        "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]
      .pre-commit-config.yaml

    ----- stderr -----
    "#);
}
