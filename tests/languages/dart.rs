#![allow(clippy::needless_raw_string_hashes)]

use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

#[test]
fn health_check() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: dart --version
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Dart SDK version: [VERSION]

    ----- stderr -----
    ");

    // Run again to check `health_check` works correctly.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Dart SDK version: [VERSION]

    ----- stderr -----
    ");
}

/// Test specifying `language_version` for Dart hooks which is not supported.
#[test]
fn language_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dart
                entry: dart --version
                language_version: '3.0'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `local`
      caused by: Hook specified `language_version: 3.0` but the language `dart` does not support toolchain installation for now
    ");
}

/// Test that stderr from hooks is captured and shown to the user.
#[test]
fn hook_stderr() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dart
                entry: dart ./hook.dart
    "});

    context
        .work_dir()
        .child("hook.dart")
        .write_str(indoc::indoc! {r#"
            import 'dart:io';
            void main() {
              stderr.writeln('Error from Dart hook');
              exit(1);
            }
        "#})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    local....................................................................Failed
    - hook id: local
    - exit code: 1

      Error from Dart hook

    ----- stderr -----
    ");

    Ok(())
}

/// Test Dart script execution with file arguments.
#[test]
fn script_with_files() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: dart ./script.dart
                verbose: true
    "});

    context
        .work_dir()
        .child("script.dart")
        .write_str(indoc::indoc! {r#"
            import 'dart:io';
            void main(List<String> args) {
              for (var arg in args) {
                print('Processing file: $arg');
              }
            }
        "#})?;

    context
        .work_dir()
        .child("test1.dart")
        .write_str("void main() { print('test1'); }")?;

    context
        .work_dir()
        .child("test2.dart")
        .write_str("void main() { print('test2'); }")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Processing file: script.dart
      Processing file: .pre-commit-config.yaml
      Processing file: test2.dart
      Processing file: test1.dart

    ----- stderr -----
    ");

    Ok(())
}

/// Test Dart hook with pubspec.yaml and dependencies (from pre-commit test suite).
#[test]
#[ignore = "Requires complex setup with dart executables"]
fn with_pubspec_and_dependencies() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: hello-world-dart
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context
        .work_dir()
        .child("pubspec.yaml")
        .write_str(indoc::indoc! {r#"
            environment:
              sdk: '>=2.17.0 <4.0.0'

            name: hello_world_dart

            executables:
                hello-world-dart:

            dependencies:
              ansicolor: ^2.0.1
        "#})?;

    std::fs::create_dir(context.work_dir().join("bin"))?;
    context
        .work_dir()
        .child("bin")
        .child("hello-world-dart.dart")
        .write_str(indoc::indoc! {r#"
            import 'package:ansicolor/ansicolor.dart';

            void main() {
                AnsiPen pen = new AnsiPen()..red();
                print("hello hello " + pen("world"));
            }
        "#})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      hello hello world

    ----- stderr -----
    ");

    Ok(())
}

/// Test Dart hook with pubspec.yaml in repository.
#[test]
fn with_pubspec() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: dart ./bin/hello.dart
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context
        .work_dir()
        .child("pubspec.yaml")
        .write_str(indoc::indoc! {r#"
            name: test_package
            description: A test package
            version: 1.0.0
            environment:
              sdk: '>=2.17.0 <4.0.0'
        "#})?;

    std::fs::create_dir(context.work_dir().join("bin"))?;
    context
        .work_dir()
        .child("bin")
        .child("hello.dart")
        .write_str(indoc::indoc! {r#"
            void main() {
              print('Hello from Dart package!');
            }
        "#})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Hello from Dart package!

    ----- stderr -----
    ");

    Ok(())
}

/// Test Dart hook with additional dependencies.
#[test]
fn additional_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: dart ./test_path.dart
                additional_dependencies: ["path"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context
        .work_dir()
        .child("test_path.dart")
        .write_str(indoc::indoc! {r#"
            import 'package:path/path.dart' as p;
            void main() {
              var joined = p.join('foo', 'bar', 'baz.txt');
              print('Joined path: $joined');
            }
        "#})
        .unwrap();

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Joined path: foo/bar/baz.txt

    ----- stderr -----
    ");
}

/// Test Dart hook with additional dependencies specifying version.
#[test]
fn additional_dependencies_with_version() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: dart ./test_path.dart
                additional_dependencies: ["path:1.8.0"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context
        .work_dir()
        .child("test_path.dart")
        .write_str(indoc::indoc! {r#"
            import 'package:path/path.dart' as p;
            void main() {
              print('Using path package');
            }
        "#})
        .unwrap();

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Using path package

    ----- stderr -----
    ");
}

/// Test Dart environment variables (PATH and `PUB_CACHE`).
#[test]
fn dart_environment() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: dart ./env_test.dart
                always_run: true
                verbose: true
                pass_filenames: false
    "#});

    context
        .work_dir()
        .child("env_test.dart")
        .write_str(indoc::indoc! {r#"
            import 'dart:io';
            void main() {
              var pubCache = Platform.environment['PUB_CACHE'];
              if (pubCache != null) {
                print('PUB_CACHE is set: ${pubCache.isNotEmpty}');
              } else {
                print('PUB_CACHE is not set');
              }
            }
        "#})
        .unwrap();

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      PUB_CACHE is set: true

    ----- stderr -----
    ");
}

/// Test remote Dart hook from GitHub repository.
#[test]
#[ignore = "Requires prek-test-repos/dart-hooks to be created"]
fn remote_hook() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/dart-hooks
            rev: v1.0.0
            hooks:
              - id: dart-hooks
                always_run: true
                verbose: true
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart-hooks...............................................................Passed
    - hook id: dart-hooks
    - duration: [TIME]

      this is a dart remote hook

    ----- stderr -----
    ");
}
