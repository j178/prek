#[cfg(unix)]
use prek_consts::env_vars::EnvVars;
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;
use insta::assert_snapshot;
use prek_consts::PRE_COMMIT_CONFIG_YAML;

use crate::common::{TestContext, cmd_snapshot, git_cmd};

mod common;

/// Tests that `repo: builtin` hooks doesn't create hook env.
#[test]
fn builtin_hooks_not_create_env() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fix end of files.........................................................Passed

    ----- stderr -----
    ");

    let hooks_dir = context
        .home_dir()
        .join("hooks")
        .read_dir()
        .into_iter()
        .flatten()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(hooks_dir.len(), 0);
}

#[test]
fn builtin_hooks_unknown_hook() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: this-hook-does-not-exist
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: error: line 4 column 9: unknown builtin hook id `this-hook-does-not-exist`
     --> <input>:4:9
      |
    2 |   - repo: builtin
    3 |     hooks:
    4 |       - id: this-hook-does-not-exist
      |         ^ unknown builtin hook id `this-hook-does-not-exist`
    ");
}

#[test]
fn end_of_file_fixer_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @r"
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
    ");

    // Assert that the files have been corrected
    assert_snapshot!(context.read("correct_lf.txt"), @"Hello World");
    assert_snapshot!(context.read("correct_crlf.txt"), @"Hello World");
    assert_snapshot!(context.read("no_newline.txt"), @"No trailing newline");
    assert_snapshot!(context.read("multiple_lf.txt"), @"Multiple newlines");
    assert_snapshot!(context.read("multiple_crlf.txt"), @"Multiple newlines");
    assert_snapshot!(context.read("empty.txt"), @"");
    assert_snapshot!(context.read("only_newlines.txt"), @"");
    assert_snapshot!(context.read("only_win_newlines.txt"), @"");

    context.git_add(".");

    // Second run: hooks should now pass. The output will be stable.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fix end of files.........................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn file_contents_sorter_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: file-contents-sorter
                files: ^allowlist\.txt$
                args: [--ignore-case]
    "});

    let cwd = context.work_dir();
    cwd.child("allowlist.txt")
        .write_str("Banana\n\napple\nApricot\n")?;
    cwd.child("ignored.txt").write_str("zebra\nant\n")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    file contents sorter.....................................................Failed
    - hook id: file-contents-sorter
    - exit code: 1
    - files were modified by this hook

      Sorting allowlist.txt

    ----- stderr -----
    ");

    assert_snapshot!(context.read("allowlist.txt"), @r"
    apple
    Apricot
    Banana
    ");
    assert_snapshot!(context.read("ignored.txt"), @r"
    zebra
    ant
    ");

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    file contents sorter.....................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_yaml_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @"
    success: false
    exit_code: 1
    ----- stdout -----
    check yaml...............................................................Failed
    - hook id: check-yaml
    - exit code: 1

      duplicate.yaml: Failed to yaml decode (error: line 2 column 1: duplicate mapping key: a not allowed here
       --> <input>:2:1
        |
      1 | a: 1
      2 | a: 2
        | ^ duplicate mapping key: a not allowed here)
      invalid.yaml: Failed to yaml decode (error: line 1 column 5: mapping values are not allowed in this context
       --> <input>:1:5
        |
      1 | a: b: c
        |     ^ mapping values are not allowed in this context)

    ----- stderr -----
    ");

    // Fix the files
    cwd.child("invalid.yaml").write_str("a:\n  b: c")?;
    cwd.child("duplicate.yaml").write_str("a: 1\nb: 2")?;

    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check yaml...............................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_yaml_multiple_document() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-yaml
                name: allow multiple documents
                args: [ --allow-multiple-documents ]
              - id: check-yaml
                name: disallow multiple documents
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

    cmd_snapshot!(context.filters(), context.run(), @"
    success: false
    exit_code: 1
    ----- stdout -----
    allow multiple documents.................................................Passed
    disallow multiple documents..............................................Failed
    - hook id: check-yaml
    - exit code: 1

      multiple.yaml: Failed to yaml decode (error: line 4 column 1: only single YAML document expected but multiple found
       --> <input>:4:1
        |
      2 | a: 1
      3 | ---
      4 | b: 2
        | ^ only single YAML document expected but multiple found)

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_vcs_permalinks_builtin() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-vcs-permalinks
                args: [--additional-github-domain=github.example.com]
    "});

    context
        .work_dir()
        .child("links.md")
        .write_str(indoc::indoc! {r"
        https://github.com/owner/repo/blob/main/file.py#L10
        https://github.example.com/owner/repo/blob/master/src/lib.rs#L5
        https://github.com/owner/repo/blob/abcdef1234567890abcdef1234567890abcdef12/file.py#L10
    "})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check vcs permalinks.....................................................Failed
    - hook id: check-vcs-permalinks
    - exit code: 1

      links.md:1:https://github.com/owner/repo/blob/main/file.py#L10
      links.md:2:https://github.example.com/owner/repo/blob/master/src/lib.rs#L5

      Non-permanent github link detected.
      On any page on github press [y] to load a permalink.

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_json_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check json...............................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn mixed_line_ending_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    assert_snapshot!(context.read("mixed.txt"), @r"
    line1
    line2
    line3
    ");
    assert_snapshot!(context.read("only_lf.txt"), @r"
    line1
    line2
    ");
    assert_snapshot!(context.read("only_crlf.txt"), @r"
    line1
    line2
    ");

    context.git_add(".");

    // Second run: hooks should now pass.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    mixed line ending........................................................Passed

    ----- stderr -----
    ");

    // Test with --fix=no
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: mixed-line-ending
                args: ['--fix=no']
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

      mixed.txt: mixed line endings

    ----- stderr -----
    ");
    assert_snapshot!(context.read("mixed.txt"), @r"
    line1
    line2
    ");

    // Test with --fix=crlf
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    assert_snapshot!(context.read("mixed.txt"), @r"
    line1
    line2
    ");

    // Test mixed args with missing value for `--fix`
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    // Create an initial commit
    let cwd = context.work_dir();
    cwd.child("README.md").write_str("Initial commit")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check for added large files..............................................Failed
    - hook id: check-added-large-files
    - exit code: 1

      large_file.txt (2 KB) exceeds 1 KB

    ----- stderr -----
    ");

    // Commit the files
    context.git_add(".");
    context.git_commit("Add large file");

    // Create a new unstaged large file
    let unstaged_large_file = cwd.child("unstaged_large_file.txt");
    unstaged_large_file.write_binary(&[0; 2048])?; // 2KB file
    context.git_add("unstaged_large_file.txt");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb=1', '--enforce-all']
    "});

    // Second run: the hook should check all files even if not staged
    cmd_snapshot!(context.filters(), context.run().arg("--all-files"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check for added large files..............................................Failed
    - hook id: check-added-large-files
    - exit code: 1

      unstaged_large_file.txt (2 KB) exceeds 1 KB
      large_file.txt (2 KB) exceeds 1 KB

    ----- stderr -----
    ");

    context.git_rm("unstaged_large_file.txt");
    context.git_clean();

    // Test git-lfs integration
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for added large files..............................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_added_large_files_workspace_mode_respects_project_relative_lfs_paths() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config("repos: []\n");

    // Regression: builtin hooks receive project-relative filenames even in workspace mode.
    // `check-added-large-files` must therefore resolve git-lfs attributes relative to the
    // nested project root, not the workspace root.
    let app = context.work_dir().child("app");
    app.create_dir_all()?;
    // Use `--enforce-all` so this regression isolates git-lfs attribute lookup in workspace
    // mode instead of depending on the separate staged-file path filtering behavior.
    app.child(PRE_COMMIT_CONFIG_YAML)
        .write_str(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb', '1', '--enforce-all']
    "})?;
    app.child(".gitattributes")
        .write_str("*.dat filter=lfs diff=lfs merge=lfs -text")?;
    app.child("large.dat").write_binary(&[0; 2048])?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `app`:
    check for added large files..............................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_added_large_files_workspace_mode_respects_project_relative_added_files() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config("repos: []\n");

    let app = context.work_dir().child("app");
    app.create_dir_all()?;
    app.child(PRE_COMMIT_CONFIG_YAML)
        .write_str(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb', '1']
    "})?;
    app.child("large.bin").write_binary(&[0; 2048])?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Running hooks for `app`:
    check for added large files..............................................Failed
    - hook id: check-added-large-files
    - exit code: 1

      large.bin (2 KB) exceeds 1 KB

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn tracked_file_exceeds_large_file_limit() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb', '1']
    "});

    let cwd = context.work_dir();

    // Create and commit a large file
    let large_file = cwd.child("large_file.txt");
    large_file.write_binary(&[0; 2048])?; // 2KB file
    context.git_add(".");
    context.git_commit("Add large file");
    // Modify the large file
    large_file.write_binary(&[0; 4096])?; // 4KB file
    context.git_add(".");

    // Run the hook: it should pass because the file is already tracked
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for added large files..............................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn builtin_hooks_workspace_mode() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: identity
    "});

    // Subproject with built-in hooks.
    let app = context.work_dir().child("app");
    app.create_dir_all()?;
    app.child(PRE_COMMIT_CONFIG_YAML)
        .write_str(indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: identity
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @"
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

      duplicate.yaml: Failed to yaml decode (error: line 2 column 1: duplicate mapping key: a not allowed here
       --> <input>:2:1
        |
      1 | a: 1
      2 | a: 2
        | ^ duplicate mapping key: a not allowed here)
      invalid.yaml: Failed to yaml decode (error: line 1 column 5: mapping values are not allowed in this context
       --> <input>:1:5
        |
      1 | a: b: c
        |     ^ mapping values are not allowed in this context)
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
    check for added large files..............................................Failed
    - hook id: check-added-large-files
    - exit code: 1

      large.bin (2 KB) exceeds 1 KB

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

    // Manually fix the files that can't be auto-fixed.
    app.child("invalid.yaml").write_str("a:\n  b: c\n")?;
    app.child("duplicate.yaml").write_str("a: 1\nb: 2\n")?;
    app.child("invalid.json")
        .write_str(concat!(r#"{"a": 1}"#, "\n"))?;
    app.child("duplicate.json")
        .write_str(concat!(r#"{"a": 1, "b": 2}"#, "\n"))?;
    app.child("large.bin").write_binary(&[0u8; 100])?;
    context.git_add(".");

    // Second run: all hooks should now pass.
    cmd_snapshot!(context.filters(), context.run(), @"
    success: true
    exit_code: 0
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
    fix end of files.........................................................Passed
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    // Should use the literal two-space indent from `--indent=  `.
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check for broken symlinks................................................Failed
    - hook id: check-symlinks
    - exit code: 1

      broken_link.txt: Broken symlink

    ----- stderr -----
    ");

    // Remove broken symlink
    std::fs::remove_file(cwd.child("broken_link.txt").path())?;
    context.git_add(".");

    // Second run: should pass
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for broken symlinks................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
#[cfg(windows)]
fn check_symlinks_hook_windows() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
#[cfg(unix)]
fn destroyed_symlinks_hook() -> Result<()> {
    const TEST_SYMLINK: &str = "test_symlink";
    const TEST_SYMLINK_TARGET: &str = "/doesnt/really/matters";
    const TEST_FILE: &str = "test_file";
    const TEST_FILE_RENAMED: &str = "test_file_renamed";

    let source = TestContext::new();
    source.init_project();

    std::os::unix::fs::symlink(
        TEST_SYMLINK_TARGET,
        source.work_dir().child(TEST_SYMLINK).path(),
    )?;
    source
        .work_dir()
        .child(TEST_FILE)
        .write_str("some random content\n")?;
    source.git_add(".");
    source.git_commit("initial");

    let tree = git_cmd(source.work_dir())
        .arg("cat-file")
        .arg("-p")
        .arg("HEAD^{tree}")
        .output()?;
    assert!(tree.status.success());
    assert!(String::from_utf8(tree.stdout)?.contains("120000 "));

    let context = TestContext::new();
    git_cmd(context.work_dir())
        .arg("-c")
        .arg("core.symlinks=false")
        .arg("clone")
        .arg(source.work_dir().path())
        .arg(".")
        .assert()
        .success();

    git_cmd(context.work_dir())
        .args(["config", "--local", "core.symlinks", "true"])
        .assert()
        .success();
    git_cmd(context.work_dir())
        .args(["mv", TEST_FILE, TEST_FILE_RENAMED])
        .assert()
        .success();

    assert!(!context.work_dir().child(TEST_SYMLINK).path().is_symlink());

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: destroyed-symlinks
    "});

    context.git_add(TEST_SYMLINK);

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    detect destroyed symlinks................................................Failed
    - hook id: destroyed-symlinks
    - exit code: 1

      Destroyed symlinks:
      - test_symlink
      You should unstage affected files:
      	git reset HEAD -- test_symlink
      And retry commit. As a long term solution you may try to explicitly tell git that your environment does not support symlinks:
      	git config core.symlinks false

    ----- stderr -----
    ");

    context
        .work_dir()
        .child(TEST_SYMLINK)
        .write_str(&format!("{TEST_SYMLINK_TARGET}\n"))?;
    context.git_add(TEST_SYMLINK);

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    detect destroyed symlinks................................................Failed
    - hook id: destroyed-symlinks
    - exit code: 1

      Destroyed symlinks:
      - test_symlink
      You should unstage affected files:
      	git reset HEAD -- test_symlink
      And retry commit. As a long term solution you may try to explicitly tell git that your environment does not support symlinks:
      	git config core.symlinks false

    ----- stderr -----
    ");

    context
        .work_dir()
        .child(TEST_SYMLINK)
        .write_str(&format!("{}\n", "0".repeat(TEST_SYMLINK_TARGET.len())))?;
    context.git_add(TEST_SYMLINK);

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    detect destroyed symlinks................................................Passed

    ----- stderr -----
    ");

    context
        .work_dir()
        .child(TEST_SYMLINK)
        .write_str(&format!("{}\n", "0".repeat(TEST_SYMLINK_TARGET.len() + 3)))?;
    context.git_add(TEST_SYMLINK);

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    detect destroyed symlinks................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn detect_private_key_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for merge conflicts................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_merge_conflict_without_assume_flag() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Without --assume-in-merge, hook should pass even with conflict markers
    // if we're not actually in a merge state
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for merge conflicts................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_xml_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
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
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check xml................................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn no_commit_to_branch_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
    "});

    let cwd = context.work_dir();

    // Create a test file
    cwd.child("test.txt").write_str("Hello World")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    // Test 1: Try to commit to master branch (should fail)
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - exit code: 1

      You are not allowed to commit to branch 'master'

    ----- stderr -----
    ");

    // Test 2: Create and switch to a feature branch (should pass)
    context.git_branch("feature/new-feature");
    context.git_checkout("feature/new-feature");

    cwd.child("feature.txt").write_str("Feature content")?;
    context.git_add(".");
    context.git_commit("Add feature");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    // Test 3: Try to commit to main branch (should fail)
    context.git_branch("main");
    context.git_checkout("main");

    cwd.child("main.txt").write_str("Main content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - exit code: 1

      You are not allowed to commit to branch 'main'

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn no_commit_to_branch_hook_with_custom_branches() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                args: ['--branch', 'develop', '--branch', 'production']
    "});

    let cwd = context.work_dir();

    // Create a test file
    cwd.child("test.txt").write_str("Hello World")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    // Test 1: Try to commit to master branch (should pass - not in custom list)
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    // Test 2: Create and switch to develop branch (should fail)
    context.git_branch("develop");
    context.git_checkout("develop");

    cwd.child("develop.txt").write_str("Develop content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - exit code: 1

      You are not allowed to commit to branch 'develop'

    ----- stderr -----
    ");

    // Test 3: Create and switch to production branch (should fail)
    context.git_branch("production");
    context.git_checkout("production");

    cwd.child("production.txt")
        .write_str("Production content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - exit code: 1

      You are not allowed to commit to branch 'production'

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn no_commit_to_branch_hook_with_patterns() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                args: ['--pattern', '^feature/.*', '--pattern', '.*-wip$']
    "});

    let cwd = context.work_dir();

    // Create a test file
    cwd.child("test.txt").write_str("Hello World")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    // Test 1: Try to commit to master branch (should fail - If branch is not specified, branch defaults to master and main)
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - exit code: 1

      You are not allowed to commit to branch 'master'

    ----- stderr -----
    ");

    // Test 2: Create and switch to feature branch (should fail - matches pattern)
    context.git_branch("feature/new-feature");
    context.git_checkout("feature/new-feature");

    cwd.child("feature.txt").write_str("Feature content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - exit code: 1

      You are not allowed to commit to branch 'feature/new-feature'

    ----- stderr -----
    ");

    // Test 3: Create and switch to wip branch (should fail - matches pattern)
    context.git_branch("my-branch-wip");
    context.git_checkout("my-branch-wip");

    cwd.child("wip.txt").write_str("WIP content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - exit code: 1

      You are not allowed to commit to branch 'my-branch-wip'

    ----- stderr -----
    ");

    // Test 4: Create and switch to normal branch (should pass - doesn't match patterns)
    context.git_branch("normal-branch");
    context.git_checkout("normal-branch");

    cwd.child("normal.txt").write_str("Normal content")?;
    context.git_add(".");
    context.git_commit("Add normal content");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    // Test 5: Try to run with detached head pointer status (should pass - ignore this status)
    context.git_checkout("HEAD~1");
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    // Test 6: Try to commit to branch with invalid pattern (should fail - invalid pattern)
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                args: ['--pattern', '*invalid-pattern*']
    "});

    context.git_branch("invalid-branch");
    context.git_checkout("invalid-branch");

    cwd.child("invalid.txt").write_str("Invalid content")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `no-commit-to-branch`
      caused by: Failed to compile regex patterns
      caused by: Parsing error at position 0: Target of repeat operator is invalid
    ");

    Ok(())
}

#[cfg(unix)]
#[test]
fn check_executables_have_shebangs_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-executables-have-shebangs
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("script_with_shebang.sh")
        .write_str("#!/bin/bash\necho ok\n")?;
    cwd.child("script_without_shebang.sh")
        .write_str("echo missing shebang\n")?;
    cwd.child("not_executable.txt")
        .write_str("not executable\n")?;
    cwd.child("empty.sh").touch()?;

    // Mark scripts as executable
    std::fs::set_permissions(
        cwd.child("script_with_shebang.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;
    std::fs::set_permissions(
        cwd.child("script_without_shebang.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;
    std::fs::set_permissions(
        cwd.child("empty.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;

    context.git_add(".");

    // First run: should fail for script_without_shebang.sh and empty.sh
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check that executables have shebangs.....................................Failed
    - hook id: check-executables-have-shebangs
    - exit code: 1

      empty.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x empty.sh'
        If on Windows, you may also need to: 'git add --chmod=-x empty.sh'
        If it is supposed to be executable, double-check its shebang.
      script_without_shebang.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x script_without_shebang.sh'
        If on Windows, you may also need to: 'git add --chmod=-x script_without_shebang.sh'
        If it is supposed to be executable, double-check its shebang.

    ----- stderr -----
    ");

    // Fix the files: remove executable bit or add shebang
    cwd.child("script_without_shebang.sh")
        .write_str("#!/bin/sh\necho fixed\n")?;
    std::fs::set_permissions(
        cwd.child("empty.sh").path(),
        std::fs::Permissions::from_mode(0o644),
    )?;

    context.git_add(".");

    // Second run: should now pass
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check that executables have shebangs.....................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[cfg(windows)]
#[test]
fn check_executables_have_shebangs_win() -> Result<()> {
    use crate::common::git_cmd;

    let context = TestContext::new();
    context.init_project();

    let repo_path = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-executables-have-shebangs
    "});

    let cwd = context.work_dir();

    cwd.child("win_script_with_shebang.sh")
        .write_str("#!/bin/bash\necho ok\n")?;
    cwd.child("win_script_without_shebang.sh")
        .write_str("missing shebang\n")?;

    context.git_add(".");

    git_cmd(repo_path)
        .args(["update-index", "--chmod=+x", "win_script_with_shebang.sh"])
        .status()?;

    git_cmd(repo_path)
        .args([
            "update-index",
            "--chmod=+x",
            "win_script_without_shebang.sh",
        ])
        .status()?;

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check that executables have shebangs.....................................Failed
    - hook id: check-executables-have-shebangs
    - exit code: 1

      win_script_without_shebang.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x win_script_without_shebang.sh'
        If on Windows, you may also need to: 'git add --chmod=-x win_script_without_shebang.sh'
        If it is supposed to be executable, double-check its shebang.

    ----- stderr -----
    "#);

    Ok(())
}

#[cfg(unix)]
#[test]
fn check_executables_have_shebangs_various_cases() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-executables-have-shebangs
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("partial_shebang.sh")
        .write_str("#\necho partial\n")?;
    cwd.child("shebang_with_space.sh")
        .write_str("#! /bin/bash\necho ok\n")?;
    cwd.child("non_executable.txt")
        .write_str("not executable\n")?;
    cwd.child("whitespace.sh").write_str("   \n")?;
    cwd.child("invalid_shebang.sh")
        .write_str("##!/bin/bash\necho bad\n")?;

    // Mark scripts as executable
    std::fs::set_permissions(
        cwd.child("partial_shebang.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;
    std::fs::set_permissions(
        cwd.child("shebang_with_space.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;
    std::fs::set_permissions(
        cwd.child("whitespace.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;
    std::fs::set_permissions(
        cwd.child("invalid_shebang.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;
    // non_executable.txt is not marked executable

    context.git_add(".");

    // Run: should fail for partial_shebang.sh, whitespace.sh, invalid_shebang.sh
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check that executables have shebangs.....................................Failed
    - hook id: check-executables-have-shebangs
    - exit code: 1

      partial_shebang.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x partial_shebang.sh'
        If on Windows, you may also need to: 'git add --chmod=-x partial_shebang.sh'
        If it is supposed to be executable, double-check its shebang.
      invalid_shebang.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x invalid_shebang.sh'
        If on Windows, you may also need to: 'git add --chmod=-x invalid_shebang.sh'
        If it is supposed to be executable, double-check its shebang.
      whitespace.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x whitespace.sh'
        If on Windows, you may also need to: 'git add --chmod=-x whitespace.sh'
        If it is supposed to be executable, double-check its shebang.

    ----- stderr -----
    ");

    // Fix the files: add valid shebangs or remove executable bit
    cwd.child("partial_shebang.sh")
        .write_str("#!/bin/sh\necho fixed\n")?;
    cwd.child("whitespace.sh").write_str("#!/bin/sh\n")?;
    cwd.child("invalid_shebang.sh")
        .write_str("#!/bin/bash\necho fixed\n")?;

    context.git_add(".");

    // Second run: should now pass
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check that executables have shebangs.....................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[cfg(windows)]
#[test]
fn check_executables_have_shebangs_various_cases_win() -> Result<()> {
    use crate::common::git_cmd;

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-executables-have-shebangs
    "});

    let cwd = context.work_dir();

    cwd.child("partial_shebang.sh")
        .write_str("#\necho partial\n")?;
    cwd.child("shebang_with_space.sh")
        .write_str("#! /bin/bash\necho ok\n")?;
    cwd.child("non_executable.txt")
        .write_str("not executable\n")?;
    cwd.child("whitespace.sh").write_str("   \n")?;
    cwd.child("invalid_shebang.sh")
        .write_str("##!/bin/bash\necho bad\n")?;

    context.git_add(".");

    let executable_files = [
        "partial_shebang.sh",
        "shebang_with_space.sh",
        "whitespace.sh",
        "invalid_shebang.sh",
    ];

    for file in &executable_files {
        git_cmd(cwd.path())
            .args(["update-index", "--chmod=+x", file])
            .status()?;
    }

    // Run: should fail for partial_shebang.sh, whitespace.sh, invalid_shebang.sh
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check that executables have shebangs.....................................Failed
    - hook id: check-executables-have-shebangs
    - exit code: 1

      invalid_shebang.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x invalid_shebang.sh'
        If on Windows, you may also need to: 'git add --chmod=-x invalid_shebang.sh'
        If it is supposed to be executable, double-check its shebang.
      partial_shebang.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x partial_shebang.sh'
        If on Windows, you may also need to: 'git add --chmod=-x partial_shebang.sh'
        If it is supposed to be executable, double-check its shebang.
      whitespace.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x whitespace.sh'
        If on Windows, you may also need to: 'git add --chmod=-x whitespace.sh'
        If it is supposed to be executable, double-check its shebang.

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_shebang_scripts_are_executable() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-shebang-scripts-are-executable
    "});

    cwd.child("plain.txt").write_str("plain text\n")?;
    cwd.child("script.sh").write_str("#!/bin/sh\necho hi\n")?;
    cwd.child("script_exec.sh")
        .write_str("#!/bin/sh\necho hi\n")?;

    #[cfg(unix)]
    std::fs::set_permissions(
        cwd.child("script_exec.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;

    context.git_add(".");
    git_cmd(cwd.path())
        .args(["update-index", "--chmod=+x", "script_exec.sh"])
        .assert()
        .success();

    cmd_snapshot!(context.filters(), context.run(), @"
    success: false
    exit_code: 1
    ----- stdout -----
    check that scripts with shebangs are executable..........................Failed
    - hook id: check-shebang-scripts-are-executable
    - exit code: 1

      script.sh has a shebang but is not marked executable!
        If it is supposed to be executable, try: 'chmod +x script.sh'
        If on Windows, you may also need to: 'git add --chmod=+x script.sh'
        If it is not supposed to be executable, double-check its shebang is wanted.

    ----- stderr -----
    ");

    #[cfg(unix)]
    std::fs::set_permissions(
        cwd.child("script.sh").path(),
        std::fs::Permissions::from_mode(0o755),
    )?;

    git_cmd(cwd.path())
        .args(["update-index", "--chmod=+x", "script.sh"])
        .assert()
        .success();

    cmd_snapshot!(context.filters(), context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    check that scripts with shebangs are executable..........................Passed

    ----- stderr -----
    ");

    Ok(())
}

fn is_case_sensitive_filesystem(context: &TestContext) -> Result<bool> {
    let test_lower = context.work_dir().child("case_test_file.txt");
    test_lower.write_str("test")?;
    let test_upper = context.work_dir().child("CASE_TEST_FILE.txt");
    let is_sensitive = !test_upper.exists();
    fs_err::remove_file(test_lower.path())?;
    Ok(is_sensitive)
}

#[test]
fn check_case_conflict_hook() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    if !is_case_sensitive_filesystem(&context)? {
        // Skipping test on case-insensitive filesystem
        return Ok(());
    }

    // Create initial files and commit
    let cwd = context.work_dir();
    cwd.child("README.md").write_str("Initial commit")?;
    cwd.child("src/foo.txt").write_str("existing file")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
    "});

    // Try to add a file with conflicting case
    cwd.child("src/FOO.txt").write_str("conflicting case")?;
    context.git_add(".");

    // First run: should fail due to case conflict
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for case conflicts.................................................Failed
    - hook id: check-case-conflict
    - exit code: 1

      Case-insensitivity conflict found: src/FOO.txt
      Case-insensitivity conflict found: src/foo.txt

    ----- stderr -----
    "#);

    // Remove the conflicting file
    context.git_rm("src/FOO.txt");

    // Add a non-conflicting file
    cwd.child("src/bar.txt").write_str("no conflict")?;
    context.git_add(".");

    // Second run: should pass
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check for case conflicts.................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_case_conflict_directory() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    if !is_case_sensitive_filesystem(&context)? {
        // Skipping test on case-insensitive filesystem
        return Ok(());
    }

    // Create directory with file
    let cwd = context.work_dir();
    cwd.child("src/utils/helper.py").write_str("helper")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
    "});

    // Try to add a file that conflicts with directory name
    cwd.child("src/UTILS/other.py").write_str("conflict")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for case conflicts.................................................Failed
    - hook id: check-case-conflict
    - exit code: 1

      Case-insensitivity conflict found: src/UTILS
      Case-insensitivity conflict found: src/utils

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_case_conflict_among_new_files() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    if !is_case_sensitive_filesystem(&context)? {
        // Skipping test on case-insensitive filesystem
        return Ok(());
    }

    let cwd = context.work_dir();
    cwd.child("README.md").write_str("Initial")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
    "});

    // Add multiple new files with conflicting cases
    cwd.child("NewFile.txt").write_str("file 1")?;
    cwd.child("newfile.txt").write_str("file 2")?;
    cwd.child("NEWFILE.TXT").write_str("file 3")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for case conflicts.................................................Failed
    - hook id: check-case-conflict
    - exit code: 1

      Case-insensitivity conflict found: NEWFILE.TXT
      Case-insensitivity conflict found: NewFile.txt
      Case-insensitivity conflict found: newfile.txt

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_case_conflict_workspace_mode_includes_added_files() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    if !is_case_sensitive_filesystem(&context)? {
        return Ok(());
    }

    context.write_pre_commit_config("repos: []\n");

    let app = context.work_dir().child("app");
    app.create_dir_all()?;
    app.child("foo.txt").write_str("existing file")?;
    app.child("trigger.txt").write_str("tracked trigger")?;
    context.git_add(".");
    context.git_commit("Initial commit");

    app.child(PRE_COMMIT_CONFIG_YAML)
        .write_str(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
    "})?;

    app.child("FOO.txt").write_str("conflicting case")?;
    context.git_add("app/FOO.txt");

    // Regression: in workspace mode, `get_added_files()` must return paths relative to the
    // nested project root so added files still participate in conflict detection even when
    // `--files` only names some other file in that project.
    cmd_snapshot!(
        context.filters(),
        context
            .run()
            .arg("check-case-conflict")
            .arg("--files")
            .arg("app/trigger.txt"),
        @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Running hooks for `app`:
    check for case conflicts.................................................Failed
    - hook id: check-case-conflict
    - exit code: 1

      Case-insensitivity conflict found: FOO.txt
      Case-insensitivity conflict found: foo.txt

    ----- stderr -----
    "#
    );

    Ok(())
}

#[test]
fn check_json5() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-json5
    "});

    let cwd = context.work_dir();

    // Create test files
    cwd.child("valid.json5").write_str(indoc::indoc! {r"
        // This is a comment
        {
            unquotedKey: 'value', // Trailing comma
            anotherKey: 12345,
        }
    "})?;
    cwd.child("invalid_missing_comma.json5")
        .write_str(indoc::indoc! {r"
        {
            key1: 'value1'
            key2: 'value2', // Missing comma between key-value pairs
        }
    "})?;

    context.git_add(".");

    // First run: hooks should fail
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check json5..............................................................Failed
    - hook id: check-json5
    - exit code: 1

      invalid_missing_comma.json5: Failed to json5 decode (expected comma at line 3 column 5)

    ----- stderr -----
    ");

    // Fix the files
    cwd.child("invalid_missing_comma.json5")
        .write_str(indoc::indoc! {r"
        {
            key1: 'value1',
            key2: 'value2',
        }
    "})?;
    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check json5..............................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[cfg(unix)]
#[test]
fn check_illegal_windows_names() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-illegal-windows-names
    "});

    let cwd = context.work_dir();
    cwd.child("normal.txt").write_str("ok")?;
    cwd.child("CON.txt").write_str("bad")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @"
    success: false
    exit_code: 1
    ----- stdout -----
    check illegal windows names..............................................Failed
    - hook id: check-illegal-windows-names
    - exit code: 1

      CON.txt: Illegal Windows filename

    ----- stderr -----
    ");

    Ok(())
}

/// Test that builtin hooks work correctly even when a system-wide binary with the
/// same name exists on PATH (regression test for <https://github.com/j178/prek/issues/1412>).
///
/// When pre-commit-hooks is installed system-wide via pip, binaries like
/// `trailing-whitespace-fixer` are placed in PATH. These binaries have shebangs
/// (e.g., `#!/usr/bin/python3`). Before the fix, `resolve(None)` would find these
/// binaries, parse their shebangs, and corrupt argument parsing.
#[test]
#[cfg(unix)]
fn builtin_hooks_ignore_system_path_binaries() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a fake "trailing-whitespace-fixer" binary with a shebang in a temp dir.
    // This simulates `pip install pre-commit-hooks` which places such binaries in PATH.
    let fake_bin_dir = context.home_dir().child("fake_bin");
    fake_bin_dir.create_dir_all()?;

    let fake_binary = fake_bin_dir.child("trailing-whitespace-fixer");
    fake_binary.write_str("#!/usr/bin/python3\n# fake binary\n")?;
    std::fs::set_permissions(fake_binary.path(), std::fs::Permissions::from_mode(0o755))?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: trailing-whitespace
    "});

    let cwd = context.work_dir();
    cwd.child("test.txt").write_str("hello world   \n")?;
    context.git_add(".");

    // Prepend the fake bin directory to PATH so the fake binary is found first.
    let original_path = EnvVars::var_os(EnvVars::PATH).unwrap_or_default();
    let mut new_path = std::ffi::OsString::from(fake_bin_dir.path());
    new_path.push(":");
    new_path.push(&original_path);

    // Run prek with the modified PATH.
    // Before the fix: this would fail with a clap argument parsing error like:
    //   "unexpected argument '/path/to/trailing-whitespace-fixer' found"
    // After the fix: this should pass because builtin hooks use split() not resolve(None).
    cmd_snapshot!(context.filters(), context.run().env("PATH", new_path), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing test.txt

    ----- stderr -----
    ");

    // Verify the file was fixed (trailing whitespace removed).
    assert_eq!(context.read("test.txt"), "hello world\n");

    Ok(())
}
