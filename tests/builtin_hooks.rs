use anyhow::Result;
use assert_fs::prelude::*;
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

    // Test mixed args with --fix crlf
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: mixed-line-ending
                args: ['--verbose', '--fix', 'crlf', '--help']
    "});
    context
        .work_dir()
        .child("mixed.txt")
        .write_str("line1\nline2\r\nline3\n")?;
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

    ----- stderr -----
    ");
    assert_snapshot!(context.read("mixed.txt"), @"line1\r\nline2\r\nline3\r\n");

    // Test mixed args with --fix crlf
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: mixed-line-ending
                args: ['--verbose', '--fix']
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
      caused by: Missing value for `--fix` argument
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
