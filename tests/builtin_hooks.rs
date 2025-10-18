use anyhow::Result;
use assert_fs::prelude::*;
use constants::CONFIG_FILE;
use insta::assert_snapshot;

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn end_of_file_fixer_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: end-of-file-fixer
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("correct_lf.txt").write_str("Hello World\n")?;
    cwd.child("correct_crlf.txt").write_str("Hello World\r\n")?;
    cwd.child("no_newline.txt")
        .write_str("No trailing newline")?;
    cwd.child("multiple_lf.txt")
        .write_str("Multiple newlines\n\n\n")?;
    cwd.child("multiple_crlf.txt")
        .write_str("Multiple newlines\r\n\r\n")?;
    cwd.child("empty.txt").touch()?;
    cwd.child("only_newlines.txt").write_str("\n\n")?;
    cwd.child("only_win_newlines.txt").write_str("\r\n\r\n")?;

    context.git_add(".");

    // First run: hooks should fail and fix the files
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1
    - files were modified by this hook
      Fixing multiple_crlf.txt
      Fixing only_newlines.txt
      Fixing only_win_newlines.txt
      Fixing no_newline.txt
      Fixing multiple_lf.txt

    ----- stderr -----
    "#);

    // Assert that the files have been corrected
    assert_snapshot!(context.read("correct_lf.txt"), @"Hello World\n");
    assert_snapshot!(context.read("correct_crlf.txt"), @"Hello World\n");
    assert_snapshot!(context.read("no_newline.txt"), @"No trailing newline\n");
    assert_snapshot!(context.read("multiple_lf.txt"), @"Multiple newlines\n");
    assert_snapshot!(context.read("multiple_crlf.txt"), @"Multiple newlines\n");
    assert_snapshot!(context.read("empty.txt"), @"");
    assert_snapshot!(context.read("only_newlines.txt"), @"\n");
    assert_snapshot!(context.read("only_win_newlines.txt"), @"\n");

    context.git_add(".");

    // Second run: hooks should now pass. The output will be stable.
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    fix end of files.........................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_yaml_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-yaml
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("valid.yaml").write_str("a: 1")?;
    cwd.child("invalid.yaml").write_str("a: b: c")?;
    cwd.child("duplicate.yaml").write_str("a: 1\na: 2")?;
    cwd.child("empty.yaml").touch()?;

    context.git_add(".");

    // First run: hooks should fail
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check yaml...............................................................Failed
    - hook id: check-yaml
    - exit code: 1
      duplicate.yaml: Failed to yaml decode (duplicate entry with key "a")
      invalid.yaml: Failed to yaml decode (mapping values are not allowed in this context at line 1 column 5)

    ----- stderr -----
    "#);

    // Fix the files
    cwd.child("invalid.yaml").write_str("a:\n  b: c")?;
    cwd.child("duplicate.yaml").write_str("a: 1\nb: 2")?;

    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check yaml...............................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

/// `--allow-multiple-documents` feature is not implemented in Rust,
/// it should work by delegating to the original Python implementation.
#[test]
fn check_yaml_multiple_document() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-yaml
                name: Python version
                args: [ --allow-multiple-documents ]
              - id: check-yaml
                name: Rust version
    "});

    context
        .work_dir()
        .child("multiple.yaml")
        .write_str(indoc::indoc! {r"
        ---
        a: 1
        ---
        b: 2
        "
        })?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Python version...........................................................Passed
    Rust version.............................................................Failed
    - hook id: check-yaml
    - exit code: 1
      multiple.yaml: Failed to yaml decode (deserializing from YAML containing more than one document is not supported)

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_json_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-json
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("valid.json").write_str(r#"{"a": 1}"#)?;
    cwd.child("invalid.json").write_str(r#"{"a": 1,}"#)?;
    cwd.child("duplicate.json")
        .write_str(r#"{"a": 1, "a": 2}"#)?;
    cwd.child("empty.json").touch()?;

    context.git_add(".");

    // First run: hooks should fail
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1
      duplicate.json: Failed to json decode (duplicate key `a` at line 1 column 12)
      invalid.json: Failed to json decode (trailing comma at line 1 column 9)

    ----- stderr -----
    ");

    // Fix the files
    cwd.child("invalid.json").write_str(r#"{"a": 1}"#)?;
    cwd.child("duplicate.json")
        .write_str(r#"{"a": 1, "b": 2}"#)?;

    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check json...............................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn mixed_line_ending_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: mixed-line-ending
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("mixed.txt")
        .write_str("line1\nline2\r\nline3\r\n")?;
    cwd.child("only_lf.txt").write_str("line1\nline2\n")?;
    cwd.child("only_crlf.txt").write_str("line1\r\nline2\r\n")?;
    cwd.child("no_endings.txt").write_str("hello world")?;
    cwd.child("empty.txt").touch()?;

    context.git_add(".");

    // First run: hooks should fail and fix the files
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    mixed line ending........................................................Failed
    - hook id: mixed-line-ending
    - exit code: 1
    - files were modified by this hook
      Fixing mixed.txt

    ----- stderr -----
    ");

    // Assert that the files have been corrected
    assert_snapshot!(context.read("mixed.txt"), @"line1\r\nline2\r\nline3\r\n");
    assert_snapshot!(context.read("only_lf.txt"), @"line1\nline2\n");
    assert_snapshot!(context.read("only_crlf.txt"), @"line1\r\nline2\r\n");

    context.git_add(".");

    // Second run: hooks should now pass.
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    mixed line ending........................................................Passed

    ----- stderr -----
    "#);

    // Test with --fix=no
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: mixed-line-ending
                args: ['--fix=no']
    "});
    context
        .work_dir()
        .child("mixed.txt")
        .write_str("line1\nline2\r\n")?;
    context.git_add(".");
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    mixed line ending........................................................Failed
    - hook id: mixed-line-ending
    - exit code: 1
      mixed.txt: mixed line endings

    ----- stderr -----
    "#);
    assert_snapshot!(context.read("mixed.txt"), @"line1\nline2\r\n");

    // Test with --fix=crlf
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: mixed-line-ending
                args: ['--fix', 'crlf']
    "});
    context
        .work_dir()
        .child("mixed.txt")
        .write_str("line1\nline2\r\n")?;
    context.git_add(".");
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    mixed line ending........................................................Failed
    - hook id: mixed-line-ending
    - exit code: 1
    - files were modified by this hook
      Fixing .pre-commit-config.yaml
      Fixing mixed.txt
      Fixing only_lf.txt

    ----- stderr -----
    ");
    assert_snapshot!(context.read("mixed.txt"), @"line1\r\nline2\r\n");

    // Test mixed args with missing value for `--fix`
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: mixed-line-ending
                args: ['--fix']
    "});
    context
        .work_dir()
        .child("mixed.txt")
        .write_str("line1\nline2\r\nline3\n")?;
    context.git_add(".");
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----
    mixed line ending........................................................
    ----- stderr -----
    error: Failed to run hook `mixed-line-ending`
      caused by: error: a value is required for '--fix <FIX>' but none was supplied
      [possible values: auto, no, lf, crlf, cr]
    ");

    Ok(())
}

#[test]
fn check_added_large_files_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    // Create an initial commit
    let cwd = context.work_dir();
    cwd.child("README.md").write_str("Initial commit")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-added-large-files
                args: ['--maxkb', '1']
    "});

    // Create test files
    cwd.child("small_file.txt").write_str("Hello World\n")?;
    let large_file = cwd.child("large_file.txt");
    large_file.write_binary(&[0; 2048])?; // 2KB file

    context.git_add(".");

    // First run: hook should fail because of the large file
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for added large files..............................................Failed
    - hook id: check-added-large-files
    - exit code: 1
      large_file.txt (2 KB) exceeds 1 KB

    ----- stderr -----
    "#);

    // Commit the files
    context.git_add(".");
    context.git_commit("Add large file");

    // Create a new unstaged large file
    let unstaged_large_file = cwd.child("unstaged_large_file.txt");
    unstaged_large_file.write_binary(&[0; 2048])?; // 2KB file
    context.git_add("unstaged_large_file.txt");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-added-large-files
                args: ['--maxkb=1', '--enforce-all']
    "});

    // Second run: the hook should check all files even if not staged
    cmd_snapshot!(context.filters(), context.run().arg("--all-files"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for added large files..............................................Failed
    - hook id: check-added-large-files
    - exit code: 1
      unstaged_large_file.txt (2 KB) exceeds 1 KB
      large_file.txt (2 KB) exceeds 1 KB

    ----- stderr -----
    "#);

    context.git_rm("unstaged_large_file.txt");
    context.git_clean();

    // Test git-lfs integration
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-added-large-files
                args: ['--maxkb=1']
    "});
    cwd.child(".gitattributes")
        .write_str("*.dat filter=lfs diff=lfs merge=lfs -text")?;
    context.git_add(".gitattributes");
    let lfs_file = cwd.child("lfs_file.dat");
    lfs_file.write_binary(&[0; 2048])?; // 2KB file
    context.git_add(".");

    // Third run: hook should pass because the large file is tracked by git-lfs
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check for added large files..............................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn builtin_hooks_workspace_mode() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: identity
    "});

    // Subproject with built-in hooks.
    let app = context.work_dir().child("app");
    app.create_dir_all()?;
    app.child(CONFIG_FILE).write_str(indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: identity
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: end-of-file-fixer
              - id: check-yaml
              - id: check-json
              - id: mixed-line-ending
              - id: trailing-whitespace
              - id: check-added-large-files
                args: ['--maxkb', '1']
    "})?;

    app.child("eof_no_newline.txt")
        .write_str("No trailing newline")?;
    app.child("eof_multiple_lf.txt").write_str("Multiple\n\n")?;
    app.child("mixed.txt").write_str("line1\nline2\r\n")?;
    app.child("trailing_ws.txt")
        .write_str("line with trailing space \n")?;
    app.child("correct.txt").write_str("All good here\n")?;

    app.child("invalid.yaml").write_str("a: b: c")?;
    app.child("duplicate.yaml").write_str("a: 1\na: 2")?;
    app.child("empty.yaml").touch()?;

    app.child("invalid.json").write_str(r#"{"a": 1,}"#)?;
    app.child("duplicate.json")
        .write_str(r#"{"a": 1, "a": 2}"#)?;
    app.child("empty.json").touch()?;

    // 2KB file to trigger check-added-large-files (1 KB threshold).
    app.child("large.bin").write_binary(&[0u8; 2048])?;

    context.git_add(".");

    // First run: expect failures and auto-fixes where applicable.
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Running hooks for `app`:
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]
      correct.txt
      invalid.yaml
      empty.json
      duplicate.json
      trailing_ws.txt
      large.bin
      eof_multiple_lf.txt
      duplicate.yaml
      empty.yaml
      mixed.txt
      invalid.json
      .pre-commit-config.yaml
      eof_no_newline.txt
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1
    - files were modified by this hook
      Fixing invalid.yaml
      Fixing duplicate.json
      Fixing eof_no_newline.txt
      Fixing eof_multiple_lf.txt
      Fixing duplicate.yaml
      Fixing invalid.json
    check yaml...............................................................Failed
    - hook id: check-yaml
    - exit code: 1
      duplicate.yaml: Failed to yaml decode (duplicate entry with key "a")
      invalid.yaml: Failed to yaml decode (mapping values are not allowed in this context at line 1 column 5)
    check json...............................................................Failed
    - hook id: check-json
    - exit code: 1
      duplicate.json: Failed to json decode (duplicate key `a` at line 1 column 12)
      invalid.json: Failed to json decode (trailing comma at line 1 column 9)
    mixed line ending........................................................Failed
    - hook id: mixed-line-ending
    - exit code: 1
    - files were modified by this hook
      Fixing mixed.txt
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    - files were modified by this hook
      Fixing trailing_ws.txt
    check for added large files..............................................Passed

    Running hooks for `.`:
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]
      app/.pre-commit-config.yaml
      app/invalid.json
      app/duplicate.yaml
      app/correct.txt
      app/mixed.txt
      app/invalid.yaml
      app/empty.yaml
      app/duplicate.json
      app/empty.json
      app/large.bin
      app/eof_no_newline.txt
      .pre-commit-config.yaml
      app/eof_multiple_lf.txt
      app/trailing_ws.txt

    ----- stderr -----
    "#);

    // Fix YAML and JSON issues, then stage.
    app.child("invalid.yaml").write_str("a:\n  b: c")?;
    app.child("duplicate.yaml").write_str("a: 1\nb: 2")?;
    app.child("invalid.json").write_str(r#"{"a": 1}"#)?;
    app.child("duplicate.json")
        .write_str(r#"{"a": 1, "b": 2}"#)?;
    context.git_add(".");

    // Second run: all hooks should pass.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Running hooks for `app`:
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]
      correct.txt
      invalid.yaml
      empty.json
      duplicate.json
      trailing_ws.txt
      large.bin
      eof_multiple_lf.txt
      duplicate.yaml
      empty.yaml
      mixed.txt
      invalid.json
      .pre-commit-config.yaml
      eof_no_newline.txt
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1
    - files were modified by this hook
      Fixing invalid.yaml
      Fixing duplicate.json
      Fixing duplicate.yaml
      Fixing invalid.json
    check yaml...............................................................Passed
    check json...............................................................Passed
    mixed line ending........................................................Passed
    trim trailing whitespace.................................................Passed
    check for added large files..............................................Passed

    Running hooks for `.`:
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]
      app/.pre-commit-config.yaml
      app/invalid.json
      app/duplicate.yaml
      app/correct.txt
      app/mixed.txt
      app/invalid.yaml
      app/empty.yaml
      app/duplicate.json
      app/empty.json
      app/large.bin
      app/eof_no_newline.txt
      .pre-commit-config.yaml
      app/eof_multiple_lf.txt
      app/trailing_ws.txt

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn fix_byte_order_marker_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: fix-byte-order-marker
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("without_bom.txt").write_str("Hello, World!")?;
    cwd.child("with_bom.txt").write_binary(&[
        0xef, 0xbb, 0xbf, b'H', b'e', b'l', b'l', b'o', b',', b' ', b'W', b'o', b'r', b'l', b'd',
        b'!',
    ])?;
    cwd.child("bom_only.txt")
        .write_binary(&[0xef, 0xbb, 0xbf])?;
    cwd.child("empty.txt").touch()?;

    context.git_add(".");

    // First run: hooks should fix files with BOM
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    fix utf-8 byte order marker..............................................Failed
    - hook id: fix-byte-order-marker
    - exit code: 1
    - files were modified by this hook
      bom_only.txt: removed byte-order marker
      with_bom.txt: removed byte-order marker

    ----- stderr -----
    ");

    // Verify the content is correct
    assert_eq!(context.read("with_bom.txt"), "Hello, World!");
    assert_eq!(context.read("bom_only.txt"), "");
    assert_eq!(context.read("without_bom.txt"), "Hello, World!");
    assert_eq!(context.read("empty.txt"), "");

    context.git_add(".");

    // Second run: all should pass now
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fix utf-8 byte order marker..............................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn pretty_format_json_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: pretty-format-json
                args: ['--autofix']
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("valid_pretty.json").write_str(
        r#"{
  "alist": [
    2,
    34,
    234
  ],
  "blah": null,
  "foo": "bar"
}
"#,
    )?;
    cwd.child("unsorted.json").write_str(
        r#"{
  "foo": "bar",
  "alist": [2, 34, 234],
  "blah": null
}
"#,
    )?;
    cwd.child("compact.json")
        .write_str(r#"{"foo":"bar","alist":[2,34,234],"blah":null}"#)?;
    cwd.child("uppercase_unicode.json").write_str(
        r#"{
  "text": "\u4E2D\u6587"
}
"#,
    )?;
    cwd.child("invalid.json").write_str(r#"{"a": 1,}"#)?;
    cwd.child("empty.json").touch()?;

    context.git_add(".");

    // First run: hooks should fail and fix the files
    let output = context.run().output()?;
    assert!(!output.status.success());
    assert_eq!(output.status.code(), Some(1));

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains(
        "pretty format json.......................................................Failed"
    ));
    assert!(stdout.contains("- hook id: pretty-format-json"));
    assert!(stdout.contains("- exit code: 1"));
    assert!(stdout.contains("- files were modified by this hook"));
    assert!(stdout.contains("Fixed & autoformatted file compact.json"));
    assert!(stdout.contains("Fixed & autoformatted file unsorted.json"));
    assert!(stdout.contains("Fixed & autoformatted file uppercase_unicode.json"));
    assert!(stdout.contains("empty.json: Failed to json parse (no element found)"));
    assert!(stdout.contains("invalid.json: Failed to json parse: trailing comma"));

    // Verify the files have been corrected
    assert_snapshot!(context.read("valid_pretty.json"), @r#"
    {
      "alist": [
        2,
        34,
        234
      ],
      "blah": null,
      "foo": "bar"
    }
    "#);
    assert_snapshot!(context.read("unsorted.json"), @r#"
    {
      "alist": [
        2,
        34,
        234
      ],
      "blah": null,
      "foo": "bar"
    }
    "#);
    assert_snapshot!(context.read("compact.json"), @r#"
    {
      "alist": [
        2,
        34,
        234
      ],
      "blah": null,
      "foo": "bar"
    }
    "#);
    assert_snapshot!(context.read("uppercase_unicode.json"), @r#"
    {
      "text": "\u4e2d\u6587"
    }
    "#);

    // Fix invalid files with proper formatting
    cwd.child("invalid.json").write_str(
        r#"{
  "a": 1
}
"#,
    )?;
    cwd.child("empty.json").write_str(
        r#"{
  "b": 2
}
"#,
    )?;

    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn pretty_format_json_with_options() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: pretty-format-json
                args: ['--autofix', '--indent=4', '--no-sort-keys']
    "});

    let cwd = context.work_dir();

    cwd.child("test.json").write_str(r#"{"z":1,"a":2,"m":3}"#)?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - exit code: 1
    - files were modified by this hook
      Fixed & autoformatted file test.json

    ----- stderr -----
    "#);

    // Keys should NOT be sorted, but indented with 4 spaces
    assert_snapshot!(context.read("test.json"), @r#"
    {
        "z": 1,
        "a": 2,
        "m": 3
    }
    "#);

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn pretty_format_json_with_top_keys() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: pretty-format-json
                args: ['--autofix', '--top-keys=version,name']
    "});

    let cwd = context.work_dir();

    cwd.child("package.json").write_str(
        r#"{"description":"test","name":"my-package","author":"me","version":"1.0.0"}"#,
    )?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - exit code: 1
    - files were modified by this hook
      Fixed & autoformatted file package.json

    ----- stderr -----
    "#);

    // "version" and "name" should be at the top
    let content = context.read("package.json");
    let lines: Vec<&str> = content.lines().collect();
    let version_pos = lines
        .iter()
        .position(|l| l.contains("\"version\""))
        .unwrap();
    let name_pos = lines.iter().position(|l| l.contains("\"name\"")).unwrap();
    let author_pos = lines.iter().position(|l| l.contains("\"author\"")).unwrap();

    assert!(version_pos < name_pos);
    assert!(name_pos < author_pos);

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}
#[test]

fn pretty_format_json_no_ensure_ascii() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: pretty-format-json
                args: ['--autofix', '--no-ensure-ascii']
    "});

    let cwd = context.work_dir();

    cwd.child("unicode.json")
        .write_str(r#"{"text":"\u4E2D\u6587\u306B\u307B\u3093\u3054"}"#)?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - exit code: 1
    - files were modified by this hook
      Fixed & autoformatted file unicode.json

    ----- stderr -----
    "#);

    // Unicode should be decoded, not escaped
    assert_snapshot!(context.read("unicode.json"), @r#"
    {
      "text": "中文にほんご"
    }
    "#);

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn pretty_format_json_tab_indent() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: pretty-format-json
                args: ['--autofix', '--indent=  ']
    "});

    let cwd = context.work_dir();

    cwd.child("test.json").write_str(r#"{"a":1,"b":2}"#)?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - exit code: 1
    - files were modified by this hook
      Fixed & autoformatted file test.json

    ----- stderr -----
    "#);

    // Should use tabs for indentation
    let content = context.read("test.json");
    assert!(content.contains("  \"a\":"));
    assert!(content.contains("  \"b\":"));

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
#[cfg(unix)]
fn check_symlinks_hook_unix() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-symlinks
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("regular.txt").write_str("regular file")?;
    cwd.child("target.txt").write_str("target content")?;

    // Create valid symlink
    std::os::unix::fs::symlink(
        cwd.child("target.txt").path(),
        cwd.child("valid_link.txt").path(),
    )?;

    // Create broken symlink
    std::os::unix::fs::symlink(
        cwd.child("nonexistent.txt").path(),
        cwd.child("broken_link.txt").path(),
    )?;

    context.git_add(".");

    // First run: should fail due to broken symlink
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for broken symlinks................................................Failed
    - hook id: check-symlinks
    - exit code: 1
      broken_link.txt: Broken symlink

    ----- stderr -----
    "#);

    // Remove broken symlink
    std::fs::remove_file(cwd.child("broken_link.txt").path())?;
    context.git_add(".");

    // Second run: should pass
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check for broken symlinks................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
#[cfg(windows)]
fn check_symlinks_hook_windows() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-symlinks
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("regular.txt").write_str("regular file")?;
    cwd.child("target.txt").write_str("target content")?;

    // Try to create valid symlink (may fail without admin/developer mode)
    let valid_link_result = std::os::windows::fs::symlink_file(
        cwd.child("target.txt").path(),
        cwd.child("valid_link.txt").path(),
    );

    // Try to create broken symlink (may fail without admin/developer mode)
    let broken_link_result = std::os::windows::fs::symlink_file(
        cwd.child("nonexistent.txt").path(),
        cwd.child("broken_link.txt").path(),
    );

    // Skip test if we can't create symlinks (insufficient permissions)
    if valid_link_result.is_err() || broken_link_result.is_err() {
        // Skipping test: insufficient permissions for symlink creation on Windows
        return Ok(());
    }

    context.git_add(".");

    // First run: should fail due to broken symlink
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for broken symlinks................................................Failed
    - hook id: check-symlinks
    - exit code: 1
      broken_link.txt: Broken symlink

    ----- stderr -----
    "#);

    // Remove broken symlink
    std::fs::remove_file(cwd.child("broken_link.txt").path())?;
    context.git_add(".");

    // Second run: should pass
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check for broken symlinks................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn detect_private_key_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: detect-private-key
    "});

    let cwd = context.work_dir();

    // Create test files - various private key types
    cwd.child("id_rsa")
        .write_str("-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY-----\n")?;
    cwd.child("id_dsa")
        .write_str("-----BEGIN DSA PRIVATE KEY-----\nAAAAA...\n-----END DSA PRIVATE KEY-----\n")?;
    cwd.child("id_ecdsa")
        .write_str("-----BEGIN EC PRIVATE KEY-----\nMHc...\n-----END EC PRIVATE KEY-----\n")?;
    cwd.child("id_ed25519").write_str(
        "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNz...\n-----END OPENSSH PRIVATE KEY-----\n",
    )?;
    cwd.child("key.ppk")
        .write_str("PuTTY-User-Key-File-2: ssh-rsa\nEncryption: none\n")?;
    cwd.child("private.asc")
        .write_str("-----BEGIN PGP PRIVATE KEY BLOCK-----\nVersion: GnuPG...\n")?;
    cwd.child("ta.key").write_str(
        "#\n# 2048 bit OpenVPN static key\n#\n-----BEGIN OpenVPN Static key V1-----\n",
    )?;
    cwd.child("doc.txt").write_str(
        "Some documentation\n\nHere is a key:\n-----BEGIN RSA PRIVATE KEY-----\ndata\n",
    )?;
    cwd.child("safe1.txt")
        .write_str("This file talks about BEGIN_RSA_PRIVATE_KEY but doesn't contain one\n")?;

    cwd.child("safe2.txt")
        .write_str("This is just a regular file\nwith some content\n")?;
    cwd.child("empty.txt").touch()?;

    context.git_add(".");

    // First run: hooks should fail due to private keys
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    detect private key.......................................................Failed
    - hook id: detect-private-key
    - exit code: 1
      Private key found: doc.txt
      Private key found: id_ecdsa
      Private key found: key.ppk
      Private key found: id_rsa
      Private key found: id_dsa
      Private key found: id_ed25519
      Private key found: ta.key
      Private key found: private.asc

    ----- stderr -----
    ");

    // Remove all private keys
    context.git_rm("id_rsa");
    context.git_rm("id_dsa");
    context.git_rm("id_ecdsa");
    context.git_rm("id_ed25519");
    context.git_rm("key.ppk");
    context.git_rm("private.asc");
    context.git_rm("ta.key");
    context.git_rm("doc.txt");
    context.git_clean();

    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    detect private key.......................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_merge_conflict_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-merge-conflict
                args: ['--assume-in-merge']
    "});

    let cwd = context.work_dir();

    // Create test files with conflict markers
    cwd.child("conflict.txt").write_str(indoc::indoc! {r"
        Before conflict
        <<<<<<< HEAD
        Our changes
        =======
        Their changes
        >>>>>>> branch
        After conflict
    "})?;

    cwd.child("clean.txt").write_str("No conflicts here\n")?;

    cwd.child("partial_conflict.txt")
        .write_str(indoc::indoc! {r"
        Some content
        <<<<<<< HEAD
        Conflicting line
    "})?;

    context.git_add(".");

    // First run: hooks should fail due to conflict markers
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for merge conflicts................................................Failed
    - hook id: check-merge-conflict
    - exit code: 1
      partial_conflict.txt:2: Merge conflict string "<<<<<<< " found
      conflict.txt:2: Merge conflict string "<<<<<<< " found
      conflict.txt:4: Merge conflict string "=======" found
      conflict.txt:6: Merge conflict string ">>>>>>> " found

    ----- stderr -----
    "#);

    // Fix the files by removing conflict markers
    cwd.child("conflict.txt").write_str(indoc::indoc! {r"
        Before conflict
        Our changes
        After conflict
    "})?;

    cwd.child("partial_conflict.txt")
        .write_str("Some content\nResolved line\n")?;

    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check for merge conflicts................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_merge_conflict_without_assume_flag() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    // Without --assume-in-merge, hook should pass even with conflict markers
    // if we're not actually in a merge state
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-merge-conflict
    "});

    let cwd = context.work_dir();

    cwd.child("conflict.txt").write_str(indoc::indoc! {r"
        <<<<<<< HEAD
        Our changes
        =======
        Their changes
        >>>>>>> branch
    "})?;

    context.git_add(".");

    // Should pass because we're not in a merge state and no --assume-in-merge flag
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check for merge conflicts................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_xml_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-xml
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("valid.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
</root>"#,
    )?;
    cwd.child("invalid_unclosed.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value
</root>"#,
    )?;
    cwd.child("invalid_mismatched.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</different>
</root>"#,
    )?;
    cwd.child("multiple_roots.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<element>value</element>
<another>value</another>"#,
    )?;
    cwd.child("empty.xml").touch()?;

    context.git_add(".");

    // First run: hooks should fail
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check xml................................................................Failed
    - hook id: check-xml
    - exit code: 1
      invalid_mismatched.xml: Failed to xml parse (ill-formed document: expected `</element>`, but `</different>` was found)
      empty.xml: Failed to xml parse (no element found)
      invalid_unclosed.xml: Failed to xml parse (ill-formed document: expected `</element>`, but `</root>` was found)
      multiple_roots.xml: Failed to xml parse (junk after document element)

    ----- stderr -----
    ");

    // Fix the files
    cwd.child("invalid_unclosed.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
</root>"#,
    )?;
    cwd.child("invalid_mismatched.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
</root>"#,
    )?;
    cwd.child("multiple_roots.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
    <another>value</another>
</root>"#,
    )?;

    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check xml................................................................Failed
    - hook id: check-xml
    - exit code: 1
      empty.xml: Failed to xml parse (no element found)

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_xml_with_features() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: check-xml
    "});

    let cwd = context.work_dir();

    // Create test files with various XML features
    cwd.child("with_attributes.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="http://example.com">
    <element id="1" type="test">value</element>
</root>"#,
    )?;
    cwd.child("with_cdata.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element><![CDATA[Some <special> characters & symbols]]></element>
</root>"#,
    )?;
    cwd.child("with_comments.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <!-- This is a comment -->
    <element>value</element>
</root>"#,
    )?;
    cwd.child("with_doctype.xml").write_str(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE root SYSTEM "root.dtd">
<root>
    <element>value</element>
</root>"#,
    )?;

    context.git_add(".");

    // All should pass
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check xml................................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}
