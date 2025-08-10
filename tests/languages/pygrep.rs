use anyhow::Result;

use assert_fs::prelude::*;

use crate::common::{TestContext, cmd_snapshot};

/// Test basic pygrep functionality - case-sensitive matching
#[test]
fn basic_case_sensitive() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py")
        .write_str("TODO: implement this\nprint('Hello World')\n# todo: fix later")?;
    cwd.child("other.py")
        .write_str("print('No issues here')\n")?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-todo
                name: check-todo
                language: pygrep
                entry: "TODO"
                files: "\\.py$"
        "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check-todo...............................................................Failed
    - hook id: check-todo
    - exit code: 1
      test.py:1:TODO: implement this

    ----- stderr -----
    "#);

    Ok(())
}

/// Test case-insensitive matching
#[test]
fn case_insensitive() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py")
        .write_str("TODO: implement this\nprint('Hello World')\n# todo: fix later")?;
    cwd.child("other.py")
        .write_str("print('No issues here')\n")?;

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r##"
    success: false
    exit_code: 1
    ----- stdout -----
    check-todo-insensitive...................................................Failed
    - hook id: check-todo-insensitive
    - exit code: 1
      test.py:1:TODO: implement this
      test.py:3:# todo: fix later

    ----- stderr -----
    "##);

    Ok(())
}

/// Test multiline mode
#[test]
fn multiline_mode() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py").write_str(
        "def function():\n    \"\"\"A function\n    with multiline docstring\n    \"\"\"\n    pass",
    )?;

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
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

    Ok(())
}

/// Test negate mode - passes when pattern is NOT found
#[test]
fn negate_mode() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("good.py").write_str("print('Hello World')\n")?;
    cwd.child("bad.py")
        .write_str("TODO: implement this\nprint('Hello World')\n")?;

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    no-todo..................................................................Failed
    - hook id: no-todo
    - exit code: 1
      good.py

    ----- stderr -----
    "#);

    Ok(())
}

/// Test negate mode with multiline - should output filename if pattern not found
#[test]
fn negate_multiline_mode() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("no_pattern.py")
        .write_str("print('Hello World')\n")?;
    cwd.child("has_pattern.py").write_str(
        "def function():\n    \"\"\"A function\n    with multiline docstring\n    \"\"\"\n    pass",
    )?;

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check-no-multiline-docstring.............................................Failed
    - hook id: check-no-multiline-docstring
    - exit code: 1
      no_pattern.py

    ----- stderr -----
    "#);

    Ok(())
}

/// Test invalid regex pattern
#[test]
fn invalid_regex() {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py")
        .write_str("print('Hello World')\n")
        .unwrap();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: invalid-regex
                name: invalid-regex
                language: pygrep
                entry: "[unclosed"
                files: "\\.py$"
        "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----
    invalid-regex............................................................
    ----- stderr -----
    error: Failed to run hook `invalid-regex`
      caused by: Failed to parse regex: unterminated character set at position 0
    "#);
}

#[test]
fn python_regex_quirks() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py")
        .write_str("def function(arg1, arg2):\n    pass\ndef bad_function():\n    pass")?;

    // Test lookbehind assertion - function with arguments
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: function-with-args
                name: function-with-args
                language: pygrep
                entry: "def\\s+\\w+\\([^)]*\\w[^)]*\\):"
                files: "\\.py$"
        "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    function-with-args.......................................................Failed
    - hook id: function-with-args
    - exit code: 1
      test.py:1:def function(arg1, arg2):

    ----- stderr -----
    "#);

    Ok(())
}

/// Test complex regex with word boundaries and character classes
#[test]
fn complex_regex_patterns() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py")
        .write_str("import sys\nfrom os import path\nimport json\nfrom typing import Dict")?;

    // Match import statements but not 'from' imports
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: direct-imports
                name: direct-imports
                language: pygrep
                entry: "^import\\s+[a-zA-Z_][a-zA-Z0-9_]*$"
                files: "\\.py$"
        "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    direct-imports...........................................................Failed
    - hook id: direct-imports
    - exit code: 1
      test.py:1:import sys
      test.py:3:import json

    ----- stderr -----
    "#);

    Ok(())
}

/// Test combination of case insensitive and multiline
#[test]
fn case_insensitive_multiline() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py")
        .write_str("# TODO: fix this\ndef function():\n    # todo: implement\n    pass")?;

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r##"
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
    "##);

    Ok(())
}

/// Test successful case where pattern is not found
#[test]
fn pattern_not_found() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py")
        .write_str("print('Hello World')\n# All good here")?;

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: check-todo
                name: check-todo
                language: pygrep
                entry: "TODO"
                files: "\\.py$"
        "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check-todo...............................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn invalid_args() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("test.py")
        .write_str("print('Hello World')\n# All good here")?;

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----
    check-todo...............................................................
    ----- stderr -----
    error: Failed to run hook `check-todo`
      caused by: Failed to parse `args`
      caused by: Unknown argument: --hello
    "#);

    Ok(())
}
