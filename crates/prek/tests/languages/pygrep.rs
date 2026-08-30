use crate::common::{TestEnv, cmd_snapshot};

/// Test basic pygrep functionality - case-sensitive matching
#[test]
fn basic_case_sensitive() {
    let context = TestEnv::new()
        .with_file(
            "test.py",
            indoc::indoc! {r"
                TODO: implement this
                print('Hello World')
                # todo: fix later"},
        )
        .with_file("other.py", "print('No issues here')\n")
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-todo
                name: check-todo
                language: pygrep
                entry: "TODO"
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check-todo...............................................................Failed
    - hook id: check-todo
    - exit code: 1

      test.py:1:TODO: implement this

    ----- stderr -----
    ");

    // Run again to ensure `health_check` works correctly.
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check-todo...............................................................Failed
    - hook id: check-todo
    - exit code: 1

      test.py:1:TODO: implement this

    ----- stderr -----
    ");
}

/// Test case-insensitive matching
#[test]
fn case_insensitive() {
    let context = TestEnv::new()
        .with_file(
            "test.py",
            indoc::indoc! {r"
                TODO: implement this
                print('Hello World')
                # todo: fix later"},
        )
        .with_file("other.py", "print('No issues here')\n")
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-todo-insensitive
                name: check-todo-insensitive
                language: pygrep
                entry: "TODO"
                args: ["--ignore-case"]
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check-todo-insensitive...................................................Failed
    - hook id: check-todo-insensitive
    - exit code: 1

      test.py:1:TODO: implement this
      test.py:3:# todo: fix later

    ----- stderr -----
    ");
}

/// Test multiline mode
#[test]
fn multiline_mode() {
    let context = TestEnv::new()
        .with_file(
            "test.py",
            indoc::indoc! {r#"
            def function():
                """A function
                with multiline docstring
                """
                pass"#},
        )
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-multiline-docstring
                name: check-multiline-docstring
                language: pygrep
                entry: '""".*\n.*docstring.*\n.*"""'
                args: ["--multiline"]
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check-multiline-docstring................................................Failed
    - hook id: check-multiline-docstring
    - exit code: 1

      test.py:2:    """A function
          with multiline docstring
          """

    ----- stderr -----
    "#);
}

/// Test negate mode - passes when pattern is NOT found
#[test]
fn negate_mode() {
    let context = TestEnv::new()
        .with_file("good.py", "print('Hello World')\n")
        .with_file("bad.py", "TODO: implement this\nprint('Hello World')\n")
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: no-todo
                name: no-todo
                language: pygrep
                entry: "TODO"
                args: ["--negate"]
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    no-todo..................................................................Failed
    - hook id: no-todo
    - exit code: 1

      good.py

    ----- stderr -----
    ");
}

/// Test negate mode with multiline - should output filename if pattern not found
#[test]
fn negate_multiline_mode() {
    let context = TestEnv::new()
        .with_file("no_pattern.py", "print('Hello World')\n")
        .with_file(
            "has_pattern.py",
            indoc::indoc! {r#"
                def function():
                    """A function
                    with multiline docstring
                    """
                    pass"#},
        )
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-no-multiline-docstring
                name: check-no-multiline-docstring
                language: pygrep
                entry: '""".*\n.*docstring.*\n.*"""'
                args: ["--multiline", "--negate"]
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check-no-multiline-docstring.............................................Failed
    - hook id: check-no-multiline-docstring
    - exit code: 1

      no_pattern.py

    ----- stderr -----
    ");
}

/// Test invalid regex pattern
#[test]
fn invalid_regex() {
    let context = TestEnv::new()
        .with_file("test.py", "print('Hello World')\n")
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: invalid-regex
                name: invalid-regex
                language: pygrep
                entry: "[unclosed"
                files: "\\.py$"
        "#})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `invalid-regex`
      caused by: Failed to parse regex: unterminated character set at position 0
    ");
}

#[test]
fn python_regex_quirks() {
    let context = TestEnv::new()
        .with_file(
            "test.py",
            indoc::indoc! {r"
            def function(arg1, arg2):
                pass
            def bad_function():
                pass"},
        )
        .init_git();

    // Test lookbehind assertion - function with arguments
    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: function-with-args
                name: function-with-args
                language: pygrep
                entry: "def\\s+\\w+\\([^)]*\\w[^)]*\\):"
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    function-with-args.......................................................Failed
    - hook id: function-with-args
    - exit code: 1

      test.py:1:def function(arg1, arg2):

    ----- stderr -----
    ");
}

/// Test complex regex with word boundaries and character classes
#[test]
fn complex_regex_patterns() {
    let context = TestEnv::new()
        .with_file(
            "test.py",
            indoc::indoc! {r"
            import sys
            from os import path
            import json
            from typing import Dict"},
        )
        .init_git();

    // Match import statements but not 'from' imports
    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: direct-imports
                name: direct-imports
                language: pygrep
                entry: "^import\\s+[a-zA-Z_][a-zA-Z0-9_]*$"
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    direct-imports...........................................................Failed
    - hook id: direct-imports
    - exit code: 1

      test.py:1:import sys
      test.py:3:import json

    ----- stderr -----
    ");
}

/// Test combination of case insensitive and multiline
#[test]
fn case_insensitive_multiline() {
    let context = TestEnv::new()
        .with_file(
            "test.py",
            indoc::indoc! {r"
            # TODO: fix this
            def function():
                # todo: implement
                pass"},
        )
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-todos
                name: check-todos
                language: pygrep
                entry: "todo.*\n.*implement"
                args: ["--ignore-case", "--multiline"]
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check-todos..............................................................Failed
    - hook id: check-todos
    - exit code: 1

      test.py:1:# TODO: fix this
      def function():
          # todo: implement

    ----- stderr -----
    ");
}

/// Test successful case where pattern is not found
#[test]
fn pattern_not_found() {
    let context = TestEnv::new()
        .with_file("test.py", "print('Hello World')\n# All good here")
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-todo
                name: check-todo
                language: pygrep
                entry: "TODO"
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check-todo...............................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn invalid_args() {
    let context = TestEnv::new()
        .with_file("test.py", "print('Hello World')\n# All good here")
        .init_git();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-todo
                name: check-todo
                language: pygrep
                entry: "TODO"
                args: ["--hello"]
                files: "\\.py$"
        "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `check-todo`
      caused by: Failed to parse `args`
      caused by: Unknown argument: --hello
    ");
}
