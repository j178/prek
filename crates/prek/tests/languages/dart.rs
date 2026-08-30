use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn language_version() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `local`
      caused by: Hook specified `language_version: 3.0` but the language `dart` does not support toolchain installation for now
    ");
}

#[test]
fn hook_stderr() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local
                name: local
                language: dart
                entry: dart ./hook.dart
    "})
        .with_file(
            "hook.dart",
            indoc::indoc! {r"
            import 'dart:io';
            void main() {
              stderr.writeln('Error from Dart hook');
              exit(1);
            }
        "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    local....................................................................Failed
    - hook id: local
    - exit code: 1

      Error from Dart hook

    ----- stderr -----
    ");
}

#[test]
fn script_with_files() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: dart ./script.dart
                verbose: true
    "})
        .with_file(
            "script.dart",
            indoc::indoc! {r"
            import 'dart:io';
            void main(List<String> args) {
              for (var arg in args) {
                print('Processing file: $arg');
              }
            }
        "},
        )
        .with_file("test1.dart", "void main() { print('test1'); }")
        .with_file("test2.dart", "void main() { print('test2'); }")
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Processing file: .pre-commit-config.yaml
      Processing file: script.dart
      Processing file: test2.dart
      Processing file: test1.dart

    ----- stderr -----
    ");
}

#[test]
fn with_pubspec_and_dependencies() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .with_file(
            "pubspec.yaml",
            indoc::indoc! {r"
            environment:
              sdk: '>=2.17.0 <4.0.0'

            name: hello_world_dart

            executables:
                hello-world-dart:

            dependencies:
              ansicolor: ^2.0.1
        "},
        )
        .with_file(
            "bin/hello-world-dart.dart",
            indoc::indoc! {r#"
            import 'package:ansicolor/ansicolor.dart';

            void main() {
                AnsiPen pen = new AnsiPen()..red();
                print("hello hello " + pen("world"));
            }
        "#},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      hello hello world

    ----- stderr -----
    ");
}

#[test]
fn with_pubspec() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .with_file(
            "pubspec.yaml",
            indoc::indoc! {r"
            name: test_package
            description: A test package
            version: 1.0.0
            environment:
              sdk: '>=2.17.0 <4.0.0'
        "},
        )
        .with_file(
            "bin/hello.dart",
            indoc::indoc! {r"
            void main() {
              print('Hello from Dart package!');
            }
        "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Hello from Dart package!

    ----- stderr -----
    ");

    assert!(
        !context.work_dir().path().join(".dart_tool").exists(),
        "Dart hooks should not mutate the checkout with .dart_tool"
    );
}

#[test]
fn with_pubspec_and_additional_dependencies() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: dart ./bin/hello.dart
                additional_dependencies: ["path"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#})
        .with_file(
            "pubspec.yaml",
            indoc::indoc! {r"
            name: test_package
            description: A test package
            version: 1.0.0
            environment:
              sdk: '>=2.17.0 <4.0.0'
        "},
        )
        .with_file(
            "lib/greeting.dart",
            indoc::indoc! {r"
            String greet(String subject) => 'Hello $subject!';
        "},
        )
        .with_file(
            "bin/hello.dart",
            indoc::indoc! {r"
            import 'package:path/path.dart' as p;
            import 'package:test_package/greeting.dart';

            void main() {
              print(greet(p.posix.join('Dart', 'Hooks')));
            }
        "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      Hello Dart/Hooks!

    ----- stderr -----
    ");

    assert!(
        !context.work_dir().path().join(".dart_tool").exists(),
        "Dart hooks should not mutate the checkout with .dart_tool"
    );
}

#[test]
fn additional_dependencies() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
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
    "#})
        .with_file(
            "test_path.dart",
            indoc::indoc! {r"
            import 'package:path/path.dart' as p;
            void main() {
              var joined = p.join('foo', 'bar', 'baz.txt');
              print('Joined path: $joined');
            }
        "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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

#[test]
fn additional_dependencies_with_version() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
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
    "#})
        .with_file(
            "test_path.dart",
            indoc::indoc! {r"
            import 'package:path/path.dart' as p;
            void main() {
              print('Using path package');
            }
        "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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

#[test]
fn executable_alias() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: dart
                name: dart
                language: dart
                entry: cli
                always_run: true
                verbose: true
                pass_filenames: false
    "})
        .with_file(
            "pubspec.yaml",
            indoc::indoc! {r"
            name: aliased_dart_tool
            environment:
              sdk: '>=2.17.0 <4.0.0'

            executables:
              cli: hello
        "},
        )
        .with_file(
            "bin/hello.dart",
            indoc::indoc! {r"
            void main() {
              print('alias executable works');
            }
        "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    dart.....................................................................Passed
    - hook id: dart
    - duration: [TIME]

      alias executable works

    ----- stderr -----
    ");
}

#[test]
fn dart_environment() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .with_file(
            "env_test.dart",
            indoc::indoc! {r"
            import 'dart:io';
            void main() {
              var pubCache = Platform.environment['PUB_CACHE'];
              if (pubCache != null) {
                print('PUB_CACHE is set: ${pubCache.isNotEmpty}');
              } else {
                print('PUB_CACHE is not set');
              }
            }
        "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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

#[test]
fn remote_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-ci/dart-hooks
            rev: v1.1.0
            hooks:
              - id: dart-hooks
                always_run: true
                verbose: true
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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
