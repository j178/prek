#[cfg(feature = "ci")]
use assert_fs::assert::PathAssert;
#[cfg(feature = "ci")]
use assert_fs::fixture::PathChild;

use crate::common::{TestEnv, cmd_snapshot};

/// Test `language_version` parsing and installation for golang hooks.
/// We use `setup-go` action to install go 1.24 in CI, so go 1.23 will be auto downloaded.
#[cfg(feature = "ci")]
#[test]
fn language_version() -> anyhow::Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    let go_dir = context.home_dir().child("tools").child("go");
    go_dir.assert(predicates::path::missing());

    let context = context.with_filter(
        r"go version (go1\.\d{1,2})\.\d{1,2} ([\w]+/[\w]+)",
        "go version $1.X [OS]/[ARCH]",
    );

    cmd_snapshot!(context, context.run().arg("-v"), @r#"
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
    let context = TestEnv::new().init_git();

    // Run hooks with system found go.
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-ci/golang-hooks
            rev: v1.0
            hooks:
              - id: echo
                verbose: true
        "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
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
    context.write_config(indoc::indoc! {r#"
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
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
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
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-ci/golang-hooks
            rev: v1.0
            hooks:
              - id: echo
                verbose: true
                language_version: '1.23.11' # will auto download
        "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
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
fn local_additional_deps() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "go-hook",
            indoc::indoc! {r"
                - id: go-hook
                  name: go-hook
                  entry: cmd
                  language: golang
                  additional_dependencies: [ ./cmd ]
            "},
        )
        .with_file(
            "go.mod",
            indoc::indoc! {r"
                module example.com/go-hook
            "},
        )
        .with_file(
            "main.go",
            indoc::indoc! {r#"
                package main

                func main() {
                    println("Hello, World!")
                }
            "#},
        )
        .with_file(
            "cmd/main.go",
            indoc::indoc! {r#"
                package main

                func main() {
                    println("Hello, Utility!")
                }
            "#},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {hook_repo}
            rev: v1.0.0
            hooks:
              - id: go-hook
                verbose: true
   "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    go-hook..................................................................Passed
    - hook id: go-hook
    - duration: [TIME]

      Hello, Utility!

    ----- stderr -----
    ");
}

/// Ensure `go.mod` metadata (go/toolchain directives) is used to constrain
/// the Go version for remote hooks.
#[test]
fn remote_go_mod_metadata_sets_language_version() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "go-hook",
            indoc::indoc! {r"
                - id: echo
                  name: echo
                  entry: echo
                  language: golang
                  verbose: true
            "},
        )
        .with_file(
            "go.mod",
            indoc::indoc! {r"
                module example.com/go-hook

                go 2.100 // unrealistic version to ensure the downloading fails
            "},
        )
        .build();
    context.write_config(indoc::formatdoc! {r"
      repos:
        - repo: {hook_repo}
          rev: v1.0.0
          hooks:
            - id: echo
              verbose: true
      "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to install hook `echo`
      caused by: Failed to install go
      caused by: Failed to resolve go version `>= 2.100.0`
      caused by: Version `>= 2.100.0` not found on remote
    ");
}
