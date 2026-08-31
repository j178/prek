#[cfg(unix)]
use prek_consts::env_vars::{EnvVars, EnvVarsRead};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;

use anyhow::Result;
#[cfg(unix)]
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;
use insta::assert_snapshot;

use crate::common::{TestEnv, cmd_snapshot, make_executable};

mod common;

/// Tests that `repo: builtin` hooks doesn't create hook env.
#[test]
fn builtin_hooks_not_create_env() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: this-hook-does-not-exist
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @"
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
fn deny_filename_pattern_hook_matches_only_basename() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: deny-filename-pattern
                args: [--ignore-case, 'readme']
                files: '\.md$'
    "})
        .with_files([
            ("docs/README.md", ""),
            ("README/guide.md", ""),
            ("docs/guide.md", ""),
        ])
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    deny filename patterns...................................................Failed
    - hook id: deny-filename-pattern
    - description: Fails if any selected filename matches a regular expression
    - exit code: 1

      docs/README.md: filename matches a denied pattern

    ----- stderr -----
    ");
}

#[test]
fn deny_pattern_hook_reports_matching_lines() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: deny-pattern
                args:
                  - --ignore-case
                  - '\btodo\b'
                  - 'remove'
                  - '^#import\s+.+:\s+\*$'
                files: '\.typ$'
    "})
        .with_file(
            "policy.typ",
            indoc::indoc! {"
        permitted content
        TODO: remove this
        #import package: *
    "},
        )
        .with_file("ignored.txt", "TODO: ignored by files filter\n")
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    deny patterns............................................................Failed
    - hook id: deny-pattern
    - description: Fails if any file contains a matching regular expression
    - exit code: 1

      policy.typ:2:TODO: remove this
      policy.typ:3:#import package: *

    ----- stderr -----
    ");
}

#[test]
fn deny_pattern_hook_rejects_invalid_regex() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: deny-pattern
                args: ['*invalid-pattern*']
    "})
        .with_file("file.txt", "content\n")
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `deny-pattern`
      caused by: Failed to compile regex patterns
      caused by: error parsing pattern 0
      caused by: regex parse error:
        *invalid-pattern*
        ^
    error: repetition operator missing expression
    "#);
}

#[test]
fn deny_pattern_hook_reports_earliest_multiline_match() {
    let context = TestEnv::new().init_git();

    // `END` is listed first, but `BEGIN.*END` starts earlier in the file.
    // Multiline matching should report the earliest match, not the first pattern.
    let context = context
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: deny-pattern
                args: [-m, 'END', 'BEGIN.*END']
                files: '\.txt$'
    "})
        .with_file(
            "block.txt",
            indoc::indoc! {"
        before
        BEGIN
        middle
        END
        after
    "},
        );

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    deny patterns............................................................Failed
    - hook id: deny-pattern
    - description: Fails if any file contains a matching regular expression
    - exit code: 1

      block.txt:2:BEGIN
      middle
      END

    ----- stderr -----
    ");
}

#[test]
fn require_filename_pattern_hook_accepts_any_pattern_for_basename() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: require-filename-pattern
                args:
                  - '^test_.*\.py$'
                  - '^__init__\.py$'
                  - '^conftest\.py$'
                files: '(^|/)tests/.+\.py$'
    "})
        .with_files([
            ("tests/unit/test_parser.py", ""),
            ("tests/unit/parser_test.py", ""),
            ("tests/unit/__init__.py", ""),
            ("tests/unit/conftest.py", ""),
            ("tests/test_unit/parser.py", ""),
        ])
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    require filename patterns................................................Failed
    - hook id: require-filename-pattern
    - description: Fails if any selected filename does not match a regular expression
    - exit code: 1

      tests/test_unit/parser.py: filename does not match any required pattern
      tests/unit/parser_test.py: filename does not match any required pattern

    ----- stderr -----
    ");
}

#[test]
fn require_pattern_hook_reports_files_without_any_match() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: require-pattern
                args: [--ignore-case, --multiline, 'begin.*end', 'copyright']
                files: '\.txt$'
    "})
        .with_file("block.txt", "BEGIN\nmiddle\nEND\n")
        .with_file("copyright.txt", "Copyright 2026\n")
        .with_file("missing.txt", "No required marker\n")
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    require patterns.........................................................Failed
    - hook id: require-pattern
    - description: Fails if any file does not contain a matching regular expression
    - exit code: 1

      missing.txt: no pattern matched

    ----- stderr -----
    ");
}

#[test]
fn end_of_file_fixer_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
    "})
        .with_files([
            ("correct_lf.txt", "Hello World\n"),
            ("correct_crlf.txt", "Hello World\r\n"),
            ("no_newline.txt", "No trailing newline"),
            ("multiple_lf.txt", "Multiple newlines\n\n\n"),
            ("multiple_crlf.txt", "Multiple newlines\r\n\r\n"),
            ("empty.txt", ""),
            ("only_newlines.txt", "\n\n"),
            ("only_win_newlines.txt", "\r\n\r\n"),
        ])
        .init_git();

    // First run: hooks should fail and fix the files
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - description: Ensures that a file is either empty, or ends with one newline
    - exit code: 1
    - files were modified by this hook

      Fixing only_win_newlines.txt
      Fixing multiple_lf.txt
      Fixing no_newline.txt
      Fixing multiple_crlf.txt
      Fixing only_newlines.txt

    ----- stderr -----
    "#);

    // Assert that the files have been corrected
    assert_snapshot!(context.read("correct_lf.txt"), @"Hello World");
    assert_snapshot!(context.read("correct_crlf.txt"), @"Hello World");
    assert_snapshot!(context.read("no_newline.txt"), @"No trailing newline");
    assert_snapshot!(context.read("multiple_lf.txt"), @"Multiple newlines");
    assert_snapshot!(context.read("multiple_crlf.txt"), @"Multiple newlines");
    assert_snapshot!(context.read("empty.txt"), @"");
    assert_snapshot!(context.read("only_newlines.txt"), @"");
    assert_snapshot!(context.read("only_win_newlines.txt"), @"");

    context.git().add(".");

    // Second run: hooks should now pass. The output will be stable.
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fix end of files.........................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn file_contents_sorter_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: file-contents-sorter
                files: ^allowlist\.txt$
                args: [--ignore-case]
    "})
        .with_file("allowlist.txt", "Banana\n\napple\nApricot\n")
        .with_file("ignored.txt", "zebra\nant\n")
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    file contents sorter.....................................................Failed
    - hook id: file-contents-sorter
    - description: Sorts the lines in specified files (defaults to alphabetical)
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

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    file contents sorter.....................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn builtin_hook_checks_filename_from_args_after_options() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: file-contents-sorter
                args: [--ignore-case, configured.txt, configured.txt, selected.txt]
                files: ^selected\.txt$
    "})
        .with_file("configured.txt", "beta\nAlpha\n")
        .with_file("selected.txt", "Beta\nalpha\n")
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    file contents sorter.....................................................Failed
    - hook id: file-contents-sorter
    - description: Sorts the lines in specified files (defaults to alphabetical)
    - exit code: 1
    - files were modified by this hook

      Sorting configured.txt
      Sorting selected.txt

    ----- stderr -----
    ");

    assert_eq!(context.read("configured.txt"), "Alpha\nbeta\n");
    assert_eq!(context.read("selected.txt"), "alpha\nBeta\n");
}

#[test]
fn requirements_txt_fixer_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: requirements-txt-fixer
    "})
        .with_file(
            "requirements.txt",
            indoc::indoc! {"
        requests==2
        # Flask is needed by the web application.
        Flask==3
        requests==2
        pkg-resources==0.0.0
    "},
        )
        .with_file("requirements.in", "z-project\na-project\n")
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    fix requirements.txt.....................................................Failed
    - hook id: requirements-txt-fixer
    - description: Sorts entries in requirements.txt
    - exit code: 1
    - files were modified by this hook

      Sorting requirements.txt

    ----- stderr -----
    ");

    assert_eq!(
        context.read("requirements.txt"),
        indoc::indoc! {"
            # Flask is needed by the web application.
            Flask==3
            requests==2
        "}
    );
    assert_eq!(context.read("requirements.in"), "z-project\na-project\n");

    context.git().add(".");
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fix requirements.txt.....................................................Passed

    ----- stderr -----
    ");

    context.write_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: requirements-txt-fixer
              - id: check-json
        "});

    context.write_file("requirements.txt", "flask\n  requests==2\n");
    context.write_file("valid.json", "{}\n");
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    fix requirements.txt.....................................................Failed
    - hook id: requirements-txt-fixer
    - description: Sorts entries in requirements.txt
    - exit code: 1

      requirements.txt:2: requirement entry starts with whitespace
    check json...............................................................Passed

    ----- stderr -----
    ");

    assert_eq!(context.read("requirements.txt"), "flask\n  requests==2\n");
}

#[test]
fn forbid_new_submodules_hook_in_workspace_project() {
    let context = TestEnv::new()
        .with_config("repos: []\n")
        .with_file(
            "project2/.pre-commit-config.yaml",
            indoc::indoc! {r"
            repos:
              - repo: builtin
                hooks:
                  - id: forbid-new-submodules
        "},
        )
        .init_git();

    context.git().commit("Initial commit");

    let context = context.with_file("project2/sub module/README.md", "submodule\n");
    let submodule_path = context.child("project2/sub module");
    context
        .git_at(&submodule_path)
        .init()
        .add("README.md")
        .commit("Initial commit");

    context.git().run([
        "submodule",
        "add",
        "./project2/sub module",
        "project2/sub module",
    ]);

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    × project2
      forbid new submodules..................................................Failed
      - hook id: forbid-new-submodules
      - description: Prevents the addition of new Git submodules
      - exit code: 1

        sub module: new submodule introduced

        This commit introduces new git submodules.
        Did you unintentionally `git add .`?
        To fix this, run `git rm <submodule>`.
        Also check `.gitmodules` for any unintended changes.

    ----- stderr -----
    "#);
}

#[test]
fn check_yaml_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-yaml
    "})
        .with_file("valid.yaml", "a: 1")
        .with_file("invalid.yaml", "a: b: c")
        .with_file("duplicate.yaml", "a: 1\na: 2")
        .with_file("empty.yaml", "")
        .init_git();

    // First run: hooks should fail
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check yaml...............................................................Failed
    - hook id: check-yaml
    - description: Checks YAML files for parseable syntax
    - exit code: 1

      duplicate.yaml: Failed to yaml decode (duplicate mapping key: a not allowed here at line 2, column 1)
      invalid.yaml: Failed to yaml decode (mapping values are not allowed in this context at line 1, column 5)

    ----- stderr -----
    "#);

    // Fix the files
    context.write_file("invalid.yaml", "a:\n  b: c");
    context.write_file("duplicate.yaml", "a: 1\nb: 2");

    context.git().add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check yaml...............................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn check_yaml_multiple_document() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-yaml
                name: allow multiple documents
                args: [ --allow-multiple-documents ]
              - id: check-yaml
                name: disallow multiple documents
    "})
        .with_file(
            "multiple.yaml",
            indoc::indoc! {r"
        ---
        a: 1
        ---
        b: 2
        "
            },
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    allow multiple documents.................................................Passed
    disallow multiple documents..............................................Failed
    - hook id: check-yaml
    - description: Checks YAML files for parseable syntax
    - exit code: 1

      multiple.yaml: Failed to yaml decode (only single YAML document expected but multiple found at line 4, column 1)

    ----- stderr -----
    "#);
}

#[test]
fn check_vcs_permalinks_builtin() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-vcs-permalinks
                args: [--additional-github-domain=github.example.com]
    "})
        .with_file("links.md", indoc::indoc! {r"
        See https://github.com/owner/repo/blob/main/file.py#L10 and https://github.example.com/owner/repo/blob/master/src/lib.rs#L5 for context.
        https://github.com/owner/repo/blob/abcdef1234567890abcdef1234567890abcdef12/file.py#L10
    "}).init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check vcs permalinks.....................................................Failed
    - hook id: check-vcs-permalinks
    - description: Ensures that links to VCS websites are permalinks
    - exit code: 1

      Non-permanent github link detected: links.md:1:https://github.com/owner/repo/blob/main/file.py#L10
      Non-permanent github link detected: links.md:1:https://github.example.com/owner/repo/blob/master/src/lib.rs#L5

    ----- stderr -----
    ");
}

#[test]
fn check_json_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-json
    "})
        .with_file("valid.json", r#"{"a": 1}"#)
        .with_file("invalid.json", r#"{"a": 1,}"#)
        .with_file("duplicate.json", r#"{"a": 1, "a": 2}"#)
        .with_file("empty.json", "")
        .init_git();

    // First run: hooks should fail
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check json...............................................................Failed
    - hook id: check-json
    - description: Checks JSON files for parseable syntax
    - exit code: 1

      duplicate.json: Failed to json decode (duplicate key `a` at line 1 column 12)
      invalid.json: Failed to json decode (trailing comma at line 1 column 9)

    ----- stderr -----
    ");

    // Fix the files
    context.write_file("invalid.json", r#"{"a": 1}"#);
    context.write_file("duplicate.json", r#"{"a": 1, "b": 2}"#);

    context.git().add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check json...............................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn mixed_line_ending_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: mixed-line-ending
    "})
        .with_file("mixed.txt", "line1\nline2\r\nline3\r\n")
        .with_file("only_lf.txt", "line1\nline2\n")
        .with_file("only_crlf.txt", "line1\r\nline2\r\n")
        .with_file("no_endings.txt", "hello world")
        .with_file("empty.txt", "")
        .init_git();

    // First run: hooks should fail and fix the files
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    mixed line ending........................................................Failed
    - hook id: mixed-line-ending
    - description: Replaces or checks mixed line endings
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

    context.git().add(".");

    // Second run: hooks should now pass.
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    mixed line ending........................................................Passed

    ----- stderr -----
    ");

    // Test with --fix=no
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: mixed-line-ending
                args: ['--fix=no']
    "});
    context.write_file("mixed.txt", "line1\nline2\r\n");
    context.git().add(".");
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    mixed line ending........................................................Failed
    - hook id: mixed-line-ending
    - description: Replaces or checks mixed line endings
    - exit code: 1

      mixed.txt: mixed line endings

    ----- stderr -----
    ");
    assert_snapshot!(context.read("mixed.txt"), @r"
    line1
    line2
    ");

    // Test with --fix=crlf
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: mixed-line-ending
                args: ['--fix', 'crlf']
    "});
    context.write_file("mixed.txt", "line1\nline2\r\n");
    context.git().add(".");
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    mixed line ending........................................................Failed
    - hook id: mixed-line-ending
    - description: Replaces or checks mixed line endings
    - exit code: 1
    - files were modified by this hook

      Fixing .pre-commit-config.yaml
      Fixing only_lf.txt
      Fixing mixed.txt

    ----- stderr -----
    "#);
    assert_snapshot!(context.read("mixed.txt"), @r"
    line1
    line2
    ");

    // Test mixed args with missing value for `--fix`
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: mixed-line-ending
                args: ['--fix']
    "});
    context.write_file("mixed.txt", "line1\nline2\r\nline3\n");
    context.git().add(".");
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `mixed-line-ending`
      caused by: error: a value is required for '--fix <FIX>' but none was supplied
      [possible values: auto, no, lf, crlf, cr]
    ");
}

#[test]
fn check_added_large_files_hook() {
    // Create an initial commit
    let context = TestEnv::new()
        .with_file("README.md", "Initial commit")
        .init_git();
    context.git().commit("Initial commit");

    let context = context
        .with_config(indoc::indoc! {r"
            repos:
              - repo: builtin
                hooks:
                  - id: check-added-large-files
                    args: ['--maxkb', '1']
        "})
        .with_file("small_file.txt", "Hello World\n")
        .with_file("large_file.txt", [0_u8; 2048]);

    context.git().add(".");

    // First run: hook should fail because of the large file
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check for added large files..............................................Failed
    - hook id: check-added-large-files
    - description: Prevents giant files from being committed
    - exit code: 1

      large_file.txt (2 KB) exceeds 1 KB

    ----- stderr -----
    ");

    // Commit the files
    context.git().add(".").commit("Add large file");

    // Create a new unstaged large file
    context.write_file("unstaged_large_file.txt", [0_u8; 2048]);
    context.git().add("unstaged_large_file.txt");

    context.write_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb=1', '--enforce-all']
    "});

    // Second run: the hook should check all files even if not staged
    cmd_snapshot!(context, context.run().arg("--all-files"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for added large files..............................................Failed
    - hook id: check-added-large-files
    - description: Prevents giant files from being committed
    - exit code: 1

      large_file.txt (2 KB) exceeds 1 KB
      unstaged_large_file.txt (2 KB) exceeds 1 KB

    ----- stderr -----
    "#);

    context
        .git()
        .rm("unstaged_large_file.txt")
        .run(["clean", "-fdx"]);

    // Test git-lfs integration
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb=1']
        "});

    context.write_file(
        ".gitattributes",
        "*.dat filter=lfs diff=lfs merge=lfs -text",
    );
    context.git().add(".gitattributes");
    context.write_file("lfs_file.dat", [0_u8; 2048]);
    context.git().add(".");

    // Third run: hook should pass because the large file is tracked by git-lfs
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for added large files..............................................Passed

    ----- stderr -----
    ");
}

#[test]
fn check_added_large_files_workspace_mode_respects_project_relative_lfs_paths() {
    // Regression: builtin hooks receive project-relative filenames even in workspace mode.
    // `check-added-large-files` must therefore resolve git-lfs attributes relative to the
    // nested project root, not the workspace root.
    // Use `--enforce-all` so this regression isolates git-lfs attribute lookup in workspace
    // mode instead of depending on the separate staged-file path filtering behavior.
    let context = TestEnv::new()
        .with_config("repos: []\n")
        .with_project_config(
            "app",
            indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb', '1', '--enforce-all']
    "},
        )
        .with_file(
            "app/.gitattributes",
            "*.dat filter=lfs diff=lfs merge=lfs -text",
        )
        .with_file("app/large.dat", [0; 2048])
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ app
      check for added large files............................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn check_added_large_files_workspace_mode_respects_project_relative_added_files() {
    let context = TestEnv::new()
        .with_config("repos: []\n")
        .with_file(
            "app/.pre-commit-config.yaml",
            indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb', '1']
    "},
        )
        .with_file("app/large.bin", [0; 2048])
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    × app
      check for added large files............................................Failed
      - hook id: check-added-large-files
      - description: Prevents giant files from being committed
      - exit code: 1

        large.bin (2 KB) exceeds 1 KB

    ----- stderr -----
    "#);
}

#[test]
fn tracked_file_exceeds_large_file_limit() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-added-large-files
                args: ['--maxkb', '1']
    "})
        .with_file("large_file.txt", [0; 2048])
        .init_git(); // 2KB file
    context.git().add(".").commit("Add large file");
    // Modify the large file
    context.write_file("large_file.txt", [0; 4096]); // 4KB file
    context.git().add(".");

    // Run the hook: it should pass because the file is already tracked
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for added large files..............................................Passed

    ----- stderr -----
    ");
}

#[test]
fn builtin_hooks_workspace_mode() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: identity
    "})
        .with_file(
            "app/.pre-commit-config.yaml",
            indoc::indoc! {r"
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
    "},
        )
        .with_files([
            ("app/eof_no_newline.txt", "No trailing newline"),
            ("app/eof_multiple_lf.txt", "Multiple\n\n"),
            ("app/mixed.txt", "line1\nline2\r\n"),
            ("app/trailing_ws.txt", "line with trailing space \n"),
            ("app/correct.txt", "All good here\n"),
            ("app/invalid.yaml", "a: b: c"),
            ("app/duplicate.yaml", "a: 1\na: 2"),
            ("app/empty.yaml", ""),
            ("app/invalid.json", r#"{"a": 1,}"#),
            ("app/duplicate.json", r#"{"a": 1, "a": 2}"#),
            ("app/empty.json", ""),
        ])
        .with_file("app/large.bin", [0u8; 2048])
        .init_git();

    // First run: expect failures and auto-fixes where applicable.
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    × app
      identity...............................................................Passed
      - hook id: identity
      - duration: [TIME]

        invalid.json
        empty.yaml
        eof_multiple_lf.txt
        duplicate.json
        empty.json
        mixed.txt
        duplicate.yaml
        .pre-commit-config.yaml
        eof_no_newline.txt
        correct.txt
        invalid.yaml
        trailing_ws.txt
        large.bin
      fix end of files.......................................................Failed
      - hook id: end-of-file-fixer
      - description: Ensures that a file is either empty, or ends with one newline
      - exit code: 1
      - files were modified by this hook

        Fixing invalid.json
        Fixing eof_multiple_lf.txt
        Fixing duplicate.json
        Fixing duplicate.yaml
        Fixing eof_no_newline.txt
        Fixing invalid.yaml
      check yaml.............................................................Failed
      - hook id: check-yaml
      - description: Checks YAML files for parseable syntax
      - exit code: 1

        duplicate.yaml: Failed to yaml decode (duplicate mapping key: a not allowed here at line 2, column 1)
        invalid.yaml: Failed to yaml decode (mapping values are not allowed in this context at line 1, column 5)
      check json.............................................................Failed
      - hook id: check-json
      - description: Checks JSON files for parseable syntax
      - exit code: 1

        duplicate.json: Failed to json decode (duplicate key `a` at line 1 column 12)
        invalid.json: Failed to json decode (trailing comma at line 1 column 9)
      mixed line ending......................................................Failed
      - hook id: mixed-line-ending
      - description: Replaces or checks mixed line endings
      - exit code: 1
      - files were modified by this hook

        Fixing mixed.txt
      trim trailing whitespace...............................................Failed
      - hook id: trailing-whitespace
      - description: Trims trailing whitespace
      - exit code: 1
      - files were modified by this hook

        Fixing trailing_ws.txt
      check for added large files............................................Failed
      - hook id: check-added-large-files
      - description: Prevents giant files from being committed
      - exit code: 1

        large.bin (2 KB) exceeds 1 KB
    ✓ <workspace>
      identity...............................................................Passed
      - hook id: identity
      - duration: [TIME]

        app/eof_no_newline.txt
        app/empty.json
        app/empty.yaml
        app/correct.txt
        app/duplicate.yaml
        app/large.bin
        app/duplicate.json
        .pre-commit-config.yaml
        app/eof_multiple_lf.txt
        app/.pre-commit-config.yaml
        app/invalid.json
        app/mixed.txt
        app/invalid.yaml
        app/trailing_ws.txt

    ----- stderr -----
    "#);

    // Manually fix the files that can't be auto-fixed.
    context.write_file("app/invalid.yaml", "a:\n  b: c\n");
    context.write_file("app/duplicate.yaml", "a: 1\nb: 2\n");
    context.write_file("app/invalid.json", concat!(r#"{"a": 1}"#, "\n"));
    context.write_file("app/duplicate.json", concat!(r#"{"a": 1, "b": 2}"#, "\n"));
    context.write_file("app/large.bin", [0u8; 100]);
    context.git().add(".");

    // Second run: all hooks should now pass.
    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ app
      identity...............................................................Passed
      - hook id: identity
      - duration: [TIME]

        invalid.json
        empty.yaml
        eof_multiple_lf.txt
        duplicate.json
        empty.json
        mixed.txt
        duplicate.yaml
        .pre-commit-config.yaml
        eof_no_newline.txt
        correct.txt
        invalid.yaml
        trailing_ws.txt
        large.bin
      fix end of files.......................................................Passed
      check yaml.............................................................Passed
      check json.............................................................Passed
      mixed line ending......................................................Passed
      trim trailing whitespace...............................................Passed
      check for added large files............................................Passed
    ✓ <workspace>
      identity...............................................................Passed
      - hook id: identity
      - duration: [TIME]

        app/eof_no_newline.txt
        app/empty.json
        app/empty.yaml
        app/correct.txt
        app/duplicate.yaml
        app/large.bin
        app/duplicate.json
        .pre-commit-config.yaml
        app/eof_multiple_lf.txt
        app/.pre-commit-config.yaml
        app/invalid.json
        app/mixed.txt
        app/invalid.yaml
        app/trailing_ws.txt

    ----- stderr -----
    "#);
}

#[test]
fn fix_byte_order_marker_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: fix-byte-order-marker
    "})
        .with_file("without_bom.txt", "Hello, World!")
        .with_file(
            "with_bom.txt",
            [
                0xef, 0xbb, 0xbf, b'H', b'e', b'l', b'l', b'o', b',', b' ', b'W', b'o', b'r', b'l',
                b'd', b'!',
            ],
        )
        .with_file("bom_only.txt", [0xef, 0xbb, 0xbf])
        .with_file("empty.txt", "")
        .init_git();

    // First run: hooks should fix files with BOM
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    fix utf-8 byte order marker..............................................Failed
    - hook id: fix-byte-order-marker
    - description: Removes UTF-8 byte order marker
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

    context.git().add(".");

    // Second run: all should pass now
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fix utf-8 byte order marker..............................................Passed

    ----- stderr -----
    ");
}

#[test]
fn pretty_format_json_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: pretty-format-json
                args: ['--autofix']
    "})
        .with_file(
            "valid_pretty.json",
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
        )
        .with_file(
            "unsorted.json",
            r#"{
  "foo": "bar",
  "alist": [2, 34, 234],
  "blah": null
}
"#,
        )
        .with_file(
            "compact.json",
            r#"{"foo":"bar","alist":[2,34,234],"blah":null}"#,
        )
        .with_file(
            "uppercase_unicode.json",
            r#"{
  "text": "\u4E2D\u6587"
}
"#,
        )
        .with_file("invalid.json", r#"{"a": 1,}"#)
        .with_file("empty.json", "")
        .init_git();

    // First run: hooks should fail and fix the files
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - description: Checks that JSON files are pretty-formatted
    - exit code: 1
    - files were modified by this hook

      Fixing file compact.json
      Fixing file unsorted.json
      invalid.json: invalid JSON (trailing comma at line 1 column 9). Consider using the `check-json` hook.
      Fixing file uppercase_unicode.json
      empty.json: invalid JSON (EOF while parsing a value at line 1 column 0). Consider using the `check-json` hook.

    ----- stderr -----
    "#);

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
    context.write_file(
        "invalid.json",
        r#"{
  "a": 1
}
"#,
    );
    context.write_file(
        "empty.json",
        r#"{
  "b": 2
}
"#,
    );

    context.git().add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn pretty_format_json_with_options() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: pretty-format-json
                args: ['--autofix', '--indent=4', '--no-sort-keys']
    "})
        .with_file("test.json", r#"{"z":1,"a":2,"m":3}"#)
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - description: Checks that JSON files are pretty-formatted
    - exit code: 1
    - files were modified by this hook

      Fixing file test.json

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

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn pretty_format_json_with_top_keys() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: pretty-format-json
                args: ['--autofix', '--top-keys=version,name']
    "})
        .with_file(
            "package.json",
            r#"{"description":"test","name":"my-package","author":"me","version":"1.0.0"}"#,
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - description: Checks that JSON files are pretty-formatted
    - exit code: 1
    - files were modified by this hook

      Fixing file package.json

    ----- stderr -----
    "#);

    insta::assert_snapshot!(context.read("package.json"), @r#"
    {
      "version": "1.0.0",
      "name": "my-package",
      "author": "me",
      "description": "test"
    }
    "#);

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn pretty_format_json_no_ensure_ascii() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: pretty-format-json
                args: ['--autofix', '--no-ensure-ascii']
    "})
        .with_file(
            "unicode.json",
            r#"{"text":"\u4E2D\u6587\u306B\u307B\u3093\u3054"}"#,
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - description: Checks that JSON files are pretty-formatted
    - exit code: 1
    - files were modified by this hook

      Fixing file unicode.json

    ----- stderr -----
    "#);

    // Unicode should be decoded, not escaped
    assert_snapshot!(context.read("unicode.json"), @r#"
    {
      "text": "中文にほんご"
    }
    "#);

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn pretty_format_json_custom_space_indent() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: pretty-format-json
                args: ['--autofix', '--indent=  ']
    "})
        .with_file("test.json", r#"{"a":1,"b":2}"#)
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    pretty format json.......................................................Failed
    - hook id: pretty-format-json
    - description: Checks that JSON files are pretty-formatted
    - exit code: 1
    - files were modified by this hook

      Fixing file test.json

    ----- stderr -----
    "#);

    insta::assert_snapshot!(context.read("test.json"), @r#"
    {
      "a": 1,
      "b": 2
    }
    "#);

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pretty format json.......................................................Passed

    ----- stderr -----
    "#);
}

#[test]
#[cfg(unix)]
fn check_symlinks_hook_unix() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-symlinks
    "})
        .with_file("regular.txt", "regular file")
        .with_file("target.txt", "target content")
        .init_git();

    // Create valid symlink
    fs_err::os::unix::fs::symlink(
        context.child("target.txt").path(),
        context.child("valid_link.txt").path(),
    )?;

    // Create broken symlink
    fs_err::os::unix::fs::symlink(
        context.child("nonexistent.txt").path(),
        context.child("broken_link.txt").path(),
    )?;

    context.git().add(".");

    // First run: should fail due to broken symlink
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check for broken symlinks................................................Failed
    - hook id: check-symlinks
    - description: Checks for symlinks which do not point to anything
    - exit code: 1

      broken_link.txt: Broken symlink

    ----- stderr -----
    ");

    // Remove broken symlink
    fs_err::remove_file(context.child("broken_link.txt").path())?;
    context.git().add(".");

    // Second run: should pass
    cmd_snapshot!(context, context.run(), @r"
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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-symlinks
    "})
        .with_file("regular.txt", "regular file")
        .with_file("target.txt", "target content")
        .init_git();

    // Try to create valid symlink (may fail without admin/developer mode)
    let valid_link_result = fs_err::os::windows::fs::symlink_file(
        context.child("target.txt").path(),
        context.child("valid_link.txt").path(),
    );

    // Try to create broken symlink (may fail without admin/developer mode)
    let broken_link_result = fs_err::os::windows::fs::symlink_file(
        context.child("nonexistent.txt").path(),
        context.child("broken_link.txt").path(),
    );

    // Skip test if we can't create symlinks (insufficient permissions)
    if valid_link_result.is_err() || broken_link_result.is_err() {
        // Skipping test: insufficient permissions for symlink creation on Windows
        return Ok(());
    }

    context.git().add(".");

    // First run: should fail due to broken symlink
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for broken symlinks................................................Failed
    - hook id: check-symlinks
    - description: Checks for symlinks which do not point to anything
    - exit code: 1

      broken_link.txt: Broken symlink

    ----- stderr -----
    "#);

    // Remove broken symlink
    fs_err::remove_file(context.child("broken_link.txt").path())?;
    context.git().add(".");

    // Second run: should pass
    cmd_snapshot!(context, context.run(), @r#"
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

    let source = TestEnv::new()
        .with_file(TEST_FILE, "some random content\n")
        .init_git();

    fs_err::os::unix::fs::symlink(TEST_SYMLINK_TARGET, source.child(TEST_SYMLINK).path())?;
    source.git().add(".").commit("initial");

    let tree = source
        .git()
        .command()
        .arg("cat-file")
        .arg("-p")
        .arg("HEAD^{tree}")
        .output()?;
    assert!(tree.status.success());
    assert!(String::from_utf8(tree.stdout)?.contains("120000 "));

    let context = TestEnv::new();
    context
        .git()
        .command()
        .arg("-c")
        .arg("core.symlinks=false")
        .arg("clone")
        .arg(source.work_dir().path())
        .arg(".")
        .assert()
        .success();

    context
        .git()
        .run(["config", "--local", "core.symlinks", "true"])
        .run(["mv", TEST_FILE, TEST_FILE_RENAMED]);

    assert!(!context.child(TEST_SYMLINK).path().is_symlink());

    context.write_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: destroyed-symlinks
    "});

    context.git().add(TEST_SYMLINK);

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    detect destroyed symlinks................................................Failed
    - hook id: destroyed-symlinks
    - description: Detects symlinks that were replaced with regular files whose contents are the original symlink target path
    - exit code: 1

      Destroyed symlinks:
      - test_symlink
      You should unstage affected files:
      	git reset HEAD -- test_symlink
      And retry commit. As a long term solution you may try to explicitly tell git that your environment does not support symlinks:
      	git config core.symlinks false

    ----- stderr -----
    ");

    context.write_file(TEST_SYMLINK, format!("{TEST_SYMLINK_TARGET}\n"));
    context.git().add(TEST_SYMLINK);

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    detect destroyed symlinks................................................Failed
    - hook id: destroyed-symlinks
    - description: Detects symlinks that were replaced with regular files whose contents are the original symlink target path
    - exit code: 1

      Destroyed symlinks:
      - test_symlink
      You should unstage affected files:
      	git reset HEAD -- test_symlink
      And retry commit. As a long term solution you may try to explicitly tell git that your environment does not support symlinks:
      	git config core.symlinks false

    ----- stderr -----
    ");

    context.write_file(
        TEST_SYMLINK,
        format!("{}\n", "0".repeat(TEST_SYMLINK_TARGET.len())),
    );
    context.git().add(TEST_SYMLINK);

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    detect destroyed symlinks................................................Passed

    ----- stderr -----
    ");

    context.write_file(
        TEST_SYMLINK,
        format!("{}\n", "0".repeat(TEST_SYMLINK_TARGET.len() + 3)),
    );
    context.git().add(TEST_SYMLINK);

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    detect destroyed symlinks................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn detect_private_key_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: detect-private-key
    "})
        .with_file(
            "id_rsa",
            "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY-----\n",
        )
        .with_file(
            "id_dsa",
            "-----BEGIN DSA PRIVATE KEY-----\nAAAAA...\n-----END DSA PRIVATE KEY-----\n",
        )
        .with_file(
            "id_ecdsa",
            "-----BEGIN EC PRIVATE KEY-----\nMHc...\n-----END EC PRIVATE KEY-----\n",
        )
        .with_file(
            "id_ed25519",
            "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNz...\n-----END OPENSSH PRIVATE KEY-----\n",
        )
        .with_file(
            "key.ppk",
            "PuTTY-User-Key-File-2: ssh-rsa\nEncryption: none\n",
        )
        .with_file(
            "private.asc",
            "-----BEGIN PGP PRIVATE KEY BLOCK-----\nVersion: GnuPG...\n",
        )
        .with_file(
            "ta.key",
            "#\n# 2048 bit OpenVPN static key\n#\n-----BEGIN OpenVPN Static key V1-----\n",
        )
        .with_file(
            "doc.txt",
            "Some documentation\n\nHere is a key:\n-----BEGIN RSA PRIVATE KEY-----\ndata\n",
        )
        .with_file(
            "safe1.txt",
            "This file talks about BEGIN_RSA_PRIVATE_KEY but doesn't contain one\n",
        )
        .with_file(
            "safe2.txt",
            "This is just a regular file\nwith some content\n",
        )
        .with_file("empty.txt", "")
        .init_git();

    // First run: hooks should fail due to private keys
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    detect private key.......................................................Failed
    - hook id: detect-private-key
    - description: Detects the presence of private keys
    - exit code: 1

      Private key found: private.asc
      Private key found: id_ed25519
      Private key found: id_rsa
      Private key found: id_ecdsa
      Private key found: ta.key
      Private key found: id_dsa
      Private key found: key.ppk
      Private key found: doc.txt

    ----- stderr -----
    "#);

    // Remove all private keys
    context
        .git()
        .rm("id_rsa")
        .rm("id_dsa")
        .rm("id_ecdsa")
        .rm("id_ed25519")
        .rm("key.ppk")
        .rm("private.asc")
        .rm("ta.key")
        .rm("doc.txt")
        .run(["clean", "-fdx"])
        .add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    detect private key.......................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn check_merge_conflict_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-merge-conflict
                args: ['--assume-in-merge']
    "})
        .with_file(
            "conflict.txt",
            indoc::indoc! {r"
        Before conflict
        <<<<<<< HEAD
        Our changes
        =======
        Their changes
        >>>>>>> branch
        After conflict
    "},
        )
        .with_file("clean.txt", "No conflicts here\n")
        .with_file(
            "partial_conflict.txt",
            indoc::indoc! {r"
        Some content
        <<<<<<< HEAD
        Conflicting line
    "},
        )
        .with_file(
            "partial_separator_conflict.txt",
            indoc::indoc! {r"
        Some content
        <<<<<<< HEAD
        Conflicting line
        =======
    "},
        )
        .init_git();

    // First run: hooks should fail due to conflict markers
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for merge conflicts................................................Failed
    - hook id: check-merge-conflict
    - description: Checks for files that contain merge conflict strings
    - exit code: 1

      partial_conflict.txt:2: Merge conflict string "<<<<<<< " found
      conflict.txt:2: Merge conflict string "<<<<<<< " found
      conflict.txt:4: Merge conflict string "=======" found
      conflict.txt:6: Merge conflict string ">>>>>>> " found
      partial_separator_conflict.txt:2: Merge conflict string "<<<<<<< " found
      partial_separator_conflict.txt:4: Merge conflict string "=======" found

    ----- stderr -----
    "#);

    // Fix the files by removing conflict markers
    context.write_file(
        "conflict.txt",
        indoc::indoc! {r"
        Before conflict
        Our changes
        After conflict
    "},
    );

    context.write_file("partial_conflict.txt", "Some content\nResolved line\n");

    context.write_file(
        "partial_separator_conflict.txt",
        "Some content\nResolved line\n",
    );

    context.git().add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for merge conflicts................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn check_merge_conflict_ignores_rst_headings() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-merge-conflict
                args: ['--assume-in-merge']
    "})
        .with_file(
            "doc.rst",
            indoc::indoc! {r"
        Depends
        =======
    "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for merge conflicts................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn check_merge_conflict_diff3_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-merge-conflict
                args: ['--assume-in-merge']
    "})
        .with_file(
            "diff3.txt",
            indoc::indoc! {r"
        Before conflict
        <<<<<<< HEAD
        Our changes
        ||||||| base
        Common ancestor
        =======
        Their changes
        >>>>>>> branch
        After conflict
    "},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for merge conflicts................................................Failed
    - hook id: check-merge-conflict
    - description: Checks for files that contain merge conflict strings
    - exit code: 1

      diff3.txt:2: Merge conflict string "<<<<<<< " found
      diff3.txt:4: Merge conflict string "||||||| " found
      diff3.txt:6: Merge conflict string "=======" found
      diff3.txt:8: Merge conflict string ">>>>>>> " found

    ----- stderr -----
    "#);
}

#[test]
fn check_merge_conflict_without_assume_flag() {
    let context = TestEnv::new().init_git();

    // Without --assume-in-merge, hook should pass even with conflict markers
    // if we're not actually in a merge state
    let context = context
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-merge-conflict
    "})
        .with_file(
            "conflict.txt",
            indoc::indoc! {r"
        <<<<<<< HEAD
        Our changes
        =======
        Their changes
        >>>>>>> branch
    "},
        );

    context.git().add(".");

    // Should pass because we're not in a merge state and no --assume-in-merge flag
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for merge conflicts................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn check_xml_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-xml
    "})
        .with_file(
            "valid.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
</root>"#,
        )
        .with_file(
            "invalid_unclosed.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value
</root>"#,
        )
        .with_file(
            "invalid_mismatched.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</different>
</root>"#,
        )
        .with_file(
            "multiple_roots.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
<element>value</element>
<another>value</another>"#,
        )
        .with_file("empty.xml", "")
        .init_git();

    // First run: hooks should fail
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check xml................................................................Failed
    - hook id: check-xml
    - description: Checks XML files for parseable syntax
    - exit code: 1

      empty.xml: Failed to xml parse (1:1 Unexpected end of stream: no root element found)
      invalid_mismatched.xml: Failed to xml parse (3:30 Unexpected closing tag: different != element)
      multiple_roots.xml: Failed to xml parse (3:1 Unexpected token: <)
      invalid_unclosed.xml: Failed to xml parse (4:7 Unexpected closing tag: root != element)

    ----- stderr -----
    "#);

    // Fix the files
    context.write_file(
        "invalid_unclosed.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
</root>"#,
    );
    context.write_file(
        "invalid_mismatched.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
</root>"#,
    );
    context.write_file(
        "multiple_roots.xml",
        r#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
    <another>value</another>
</root>"#,
    );

    context.git().add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check xml................................................................Failed
    - hook id: check-xml
    - description: Checks XML files for parseable syntax
    - exit code: 1

      empty.xml: Failed to xml parse (1:1 Unexpected end of stream: no root element found)

    ----- stderr -----
    ");
}

#[test]
fn check_xml_with_features() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-xml
    "})
        .with_file(
            "with_attributes.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
    <root xmlns="http://example.com">
    <element id="1" type="test">value</element>
    </root>"#,
        )
        .with_file(
            "with_cdata.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
    <root>
    <element><![CDATA[Some <special> characters & symbols]]></element>
    </root>"#,
        )
        .with_file(
            "with_comments.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
    <root>
    <!-- This is a comment -->
    <element>value</element>
    </root>"#,
        )
        .with_file(
            "with_doctype.xml",
            r#"<?xml version="1.0" encoding="UTF-8"?>
    <!DOCTYPE root SYSTEM "root.dtd">
    <root>
    <element>value</element>
    </root>"#,
        )
        .init_git();

    // All should pass
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check xml................................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn no_commit_to_branch_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
    "})
        .with_file("test.txt", "Hello World")
        .init_git();

    context.git().commit("Initial commit");

    // Test 1: Try to commit to master branch (should fail)
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'master'

    ----- stderr -----
    ");

    // Test 2: Create and switch to a feature branch (should pass)
    context
        .git()
        .branch("feature/new-feature")
        .checkout("feature/new-feature");

    context.write_file("feature.txt", "Feature content");
    context.git().add(".").commit("Add feature");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    // Test 3: Try to commit to main branch (should fail)
    context.git().branch("main").checkout("main");

    context.write_file("main.txt", "Main content");
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'main'

    ----- stderr -----
    ");
}

#[test]
fn no_commit_to_branch_hook_with_custom_branches() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                args: ['--branch', 'develop', '--branch', 'production']
    "})
        .with_file("test.txt", "Hello World")
        .init_git();

    context.git().commit("Initial commit");

    // Test 1: Try to commit to master branch (should pass - not in custom list)
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    // Test 2: Create and switch to develop branch (should fail)
    context.git().branch("develop").checkout("develop");

    context.write_file("develop.txt", "Develop content");
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'develop'

    ----- stderr -----
    ");

    // Test 3: Create and switch to production branch (should fail)
    context.git().branch("production").checkout("production");

    context.write_file("production.txt", "Production content");
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'production'

    ----- stderr -----
    ");
}

#[test]
fn no_commit_to_branch_hook_with_patterns() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                args: ['--pattern', '^feature/.*', '--pattern', '.*-wip$']
    "})
        .with_file("test.txt", "Hello World")
        .init_git();

    context.git().commit("Initial commit");

    // Test 1: Try to commit to master branch (should fail - If branch is not specified, branch defaults to master and main)
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'master'

    ----- stderr -----
    ");

    // Test 2: Create and switch to feature branch (should fail - matches pattern)
    context
        .git()
        .branch("feature/new-feature")
        .checkout("feature/new-feature");

    context.write_file("feature.txt", "Feature content");
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'feature/new-feature'

    ----- stderr -----
    ");

    // Test 3: Create and switch to wip branch (should fail - matches pattern)
    context
        .git()
        .branch("my-branch-wip")
        .checkout("my-branch-wip");

    context.write_file("wip.txt", "WIP content");
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'my-branch-wip'

    ----- stderr -----
    ");

    // Test 4: Create and switch to normal branch (should pass - doesn't match patterns)
    context
        .git()
        .branch("normal-branch")
        .checkout("normal-branch");

    context.write_file("normal.txt", "Normal content");
    context.git().add(".").commit("Add normal content");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    // Test 5: Try to run with detached head pointer status (should pass - ignore this status)
    context.git().checkout("HEAD~1");
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    // Test 6: Try to commit to branch with invalid pattern (should fail - invalid pattern)
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
                args: ['--pattern', '*invalid-pattern*']
        "});

    context
        .git()
        .branch("invalid-branch")
        .checkout("invalid-branch");

    context.write_file("invalid.txt", "Invalid content");
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to run hook `no-commit-to-branch`
      caused by: Failed to compile regex pattern `*invalid-pattern*`
      caused by: Parsing error at position 0: Target of repeat operator is invalid
    ");
}

#[cfg(unix)]
#[test]
fn check_executables_have_shebangs_hook() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-executables-have-shebangs
    "})
        .with_executable_file("script_with_shebang.sh", "#!/bin/bash\necho ok\n")
        .with_executable_file("script_without_shebang.sh", "echo missing shebang\n")
        .with_file("not_executable.txt", "not executable\n")
        .with_executable_file("empty.sh", "")
        .init_git();

    // First run: should fail for script_without_shebang.sh and empty.sh
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check that executables have shebangs.....................................Failed
    - hook id: check-executables-have-shebangs
    - description: Ensures that (non-binary) executables have a shebang
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
    context.write_file("script_without_shebang.sh", "#!/bin/sh\necho fixed\n");
    fs_err::set_permissions(
        context.child("empty.sh").path(),
        std::fs::Permissions::from_mode(0o644),
    )?;

    context.git().add(".");

    // Second run: should now pass
    cmd_snapshot!(context, context.run(), @r"
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
fn check_executables_have_shebangs_win() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-executables-have-shebangs
    "})
        .with_file("win_script_with_shebang.sh", "#!/bin/bash\necho ok\n")
        .with_file("win_script_without_shebang.sh", "missing shebang\n")
        .init_git();

    context
        .git()
        .run(["update-index", "--chmod=+x", "win_script_with_shebang.sh"])
        .run([
            "update-index",
            "--chmod=+x",
            "win_script_without_shebang.sh",
        ]);

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check that executables have shebangs.....................................Failed
    - hook id: check-executables-have-shebangs
    - description: Ensures that (non-binary) executables have a shebang
    - exit code: 1

      win_script_without_shebang.sh marked executable but has no (or invalid) shebang!
        If it isn't supposed to be executable, try: 'chmod -x win_script_without_shebang.sh'
        If on Windows, you may also need to: 'git add --chmod=-x win_script_without_shebang.sh'
        If it is supposed to be executable, double-check its shebang.

    ----- stderr -----
    "#);
}

#[cfg(unix)]
#[test]
fn check_executables_have_shebangs_various_cases() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-executables-have-shebangs
    "})
        .with_executable_file("partial_shebang.sh", "#\necho partial\n")
        .with_executable_file("shebang_with_space.sh", "#! /bin/bash\necho ok\n")
        .with_file("non_executable.txt", "not executable\n")
        .with_executable_file("whitespace.sh", "   \n")
        .with_executable_file("invalid_shebang.sh", "##!/bin/bash\necho bad\n")
        .init_git();

    // Run: should fail for partial_shebang.sh, whitespace.sh, invalid_shebang.sh
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check that executables have shebangs.....................................Failed
    - hook id: check-executables-have-shebangs
    - description: Ensures that (non-binary) executables have a shebang
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

    // Fix the files: add valid shebangs or remove executable bit
    context.write_file("partial_shebang.sh", "#!/bin/sh\necho fixed\n");
    context.write_file("whitespace.sh", "#!/bin/sh\n");
    context.write_file("invalid_shebang.sh", "#!/bin/bash\necho fixed\n");

    context.git().add(".");

    // Second run: should now pass
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check that executables have shebangs.....................................Passed

    ----- stderr -----
    ");
}

#[cfg(windows)]
#[test]
fn check_executables_have_shebangs_various_cases_win() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-executables-have-shebangs
    "})
        .with_files([
            ("partial_shebang.sh", "#\necho partial\n"),
            ("shebang_with_space.sh", "#! /bin/bash\necho ok\n"),
            ("non_executable.txt", "not executable\n"),
            ("whitespace.sh", "   \n"),
            ("invalid_shebang.sh", "##!/bin/bash\necho bad\n"),
        ])
        .init_git();

    let executable_files = [
        "partial_shebang.sh",
        "shebang_with_space.sh",
        "whitespace.sh",
        "invalid_shebang.sh",
    ];

    for file in executable_files {
        context.git().run(["update-index", "--chmod=+x", file]);
    }

    // Run: should fail for partial_shebang.sh, whitespace.sh, invalid_shebang.sh
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check that executables have shebangs.....................................Failed
    - hook id: check-executables-have-shebangs
    - description: Ensures that (non-binary) executables have a shebang
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
}

#[test]
fn check_shebang_scripts_are_executable() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-shebang-scripts-are-executable
    "})
        .with_file("plain.txt", "plain text\n")
        .with_file("script.sh", "#!/bin/sh\necho hi\n")
        .with_executable_file("script_exec.sh", "#!/bin/sh\necho hi\n")
        .init_git();

    context
        .git()
        .run(["update-index", "--chmod=+x", "script_exec.sh"]);

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 1
    ----- stdout -----
    check that scripts with shebangs are executable..........................Failed
    - hook id: check-shebang-scripts-are-executable
    - description: Ensures that (non-binary) files with a shebang are executable
    - exit code: 1

      script.sh has a shebang but is not marked executable!
        If it is supposed to be executable, try: 'chmod +x script.sh'
        If on Windows, you may also need to: 'git add --chmod=+x script.sh'
        If it is not supposed to be executable, double-check its shebang is wanted.

    ----- stderr -----
    ");

    make_executable(context.child("script.sh"))?;

    context
        .git()
        .run(["update-index", "--chmod=+x", "script.sh"]);

    cmd_snapshot!(context, context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    check that scripts with shebangs are executable..........................Passed

    ----- stderr -----
    ");

    Ok(())
}

fn is_case_sensitive_filesystem(context: &TestEnv) -> Result<bool> {
    let test_lower = context.child("case_test_file.txt");
    test_lower.write_str("test")?;
    let test_upper = context.child("CASE_TEST_FILE.txt");
    let is_sensitive = !test_upper.exists();
    fs_err::remove_file(test_lower.path())?;
    Ok(is_sensitive)
}

#[test]
fn check_case_conflict_hook() -> Result<()> {
    let context = TestEnv::new().init_git();

    if !is_case_sensitive_filesystem(&context)? {
        // Skipping test on case-insensitive filesystem
        return Ok(());
    }

    // Create initial files and commit
    let context = context
        .with_file("README.md", "Initial commit")
        .with_file("src/foo.txt", "existing file");
    context.git().add(".").commit("Initial commit");

    let context = context
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
    "})
        .with_file("src/FOO.txt", "conflicting case");

    context.git().add(".");

    // First run: should fail due to case conflict
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for case conflicts.................................................Failed
    - hook id: check-case-conflict
    - description: Checks for files that would conflict in case-insensitive filesystems
    - exit code: 1

      Case-insensitivity conflict found: src/FOO.txt
      Case-insensitivity conflict found: src/foo.txt

    ----- stderr -----
    "#);

    // Remove the conflicting file
    context.git().rm("src/FOO.txt");

    // Add a non-conflicting file
    context.write_file("src/bar.txt", "no conflict");
    context.git().add(".");

    // Second run: should pass
    cmd_snapshot!(context, context.run(), @r#"
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
    let context = TestEnv::new();

    if !is_case_sensitive_filesystem(&context)? {
        // Skipping test on case-insensitive filesystem
        return Ok(());
    }

    // Create directory with file
    let context = context
        .with_file("src/utils/helper.py", "helper")
        .init_git();
    context.git().commit("Initial commit");

    let context = context
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
    "})
        .with_file("src/UTILS/other.py", "conflict");

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for case conflicts.................................................Failed
    - hook id: check-case-conflict
    - description: Checks for files that would conflict in case-insensitive filesystems
    - exit code: 1

      Case-insensitivity conflict found: src/UTILS
      Case-insensitivity conflict found: src/utils

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn check_case_conflict_among_new_files() -> Result<()> {
    let context = TestEnv::new();

    if !is_case_sensitive_filesystem(&context)? {
        // Skipping test on case-insensitive filesystem
        return Ok(());
    }

    let context = context.with_file("README.md", "Initial").init_git();
    context.git().commit("Initial commit");

    let context = context
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
    "})
        .with_file("NewFile.txt", "file 1")
        .with_file("newfile.txt", "file 2")
        .with_file("NEWFILE.TXT", "file 3");

    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    check for case conflicts.................................................Failed
    - hook id: check-case-conflict
    - description: Checks for files that would conflict in case-insensitive filesystems
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
    let context = TestEnv::new().init_git();

    if !is_case_sensitive_filesystem(&context)? {
        return Ok(());
    }

    let context = context
        .with_config("repos: []\n")
        .with_file("app/foo.txt", "existing file")
        .with_file("app/trigger.txt", "tracked trigger");
    context.git().add(".").commit("Initial commit");

    context.write_file(
        "app/.pre-commit-config.yaml",
        indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
    "},
    );

    context.write_file("app/FOO.txt", "conflicting case");
    context.git().add("app/FOO.txt");

    // Regression: in workspace mode, staged additions must be reported relative to the nested
    // project root so they still participate in conflict detection even when `--files` only
    // names some other file in that project.
    cmd_snapshot!(context,
        context
            .run()
            .arg("check-case-conflict")
            .arg("--files")
            .arg("app/trigger.txt"),
        @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    × app
      check for case conflicts...............................................Failed
      - hook id: check-case-conflict
      - description: Checks for files that would conflict in case-insensitive filesystems
      - exit code: 1

        Case-insensitivity conflict found: FOO.txt
        Case-insensitivity conflict found: foo.txt

    ----- stderr -----
    "#
    );

    Ok(())
}

#[test]
fn check_json5() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-json5
    "})
        .with_file(
            "valid.json5",
            indoc::indoc! {r"
        // This is a comment
        {
            unquotedKey: 'value', // Trailing comma
            anotherKey: 12345,
        }
    "},
        )
        .with_file(
            "invalid_missing_comma.json5",
            indoc::indoc! {r"
        {
            key1: 'value1'
            key2: 'value2', // Missing comma between key-value pairs
        }
    "},
        )
        .init_git();

    // First run: hooks should fail
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    check json5..............................................................Failed
    - hook id: check-json5
    - description: Checks JSON5 files for parseable syntax
    - exit code: 1

      invalid_missing_comma.json5: Failed to json5 decode (expected comma at line 3 column 5)

    ----- stderr -----
    ");

    // Fix the files
    context.write_file(
        "invalid_missing_comma.json5",
        indoc::indoc! {r"
        {
            key1: 'value1',
            key2: 'value2',
        }
    "},
    );
    context.git().add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check json5..............................................................Passed

    ----- stderr -----
    ");
}

#[cfg(unix)]
#[test]
fn check_illegal_windows_names() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: check-illegal-windows-names
    "})
        .with_file("normal.txt", "ok")
        .with_file("CON.txt", "bad")
        .init_git();

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 1
    ----- stdout -----
    check illegal windows names..............................................Failed
    - hook id: check-illegal-windows-names
    - description: Checks for filenames which cannot be created on Windows
    - exit code: 1

      CON.txt: Illegal Windows filename

    ----- stderr -----
    ");
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
    let context = TestEnv::new().init_git();

    // Create a fake "trailing-whitespace-fixer" binary with a shebang in a temp dir.
    // This simulates `pip install pre-commit-hooks` which places such binaries in PATH.
    let fake_bin_dir = context.home_dir().child("fake_bin");
    fake_bin_dir.create_dir_all()?;

    let fake_binary = fake_bin_dir.child("trailing-whitespace-fixer");
    fake_binary.write_str("#!/usr/bin/python3\n# fake binary\n")?;
    fs_err::set_permissions(fake_binary.path(), std::fs::Permissions::from_mode(0o755))?;

    let context = context
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: trailing-whitespace
    "})
        .with_file("test.txt", "hello world   \n");

    context.git().add(".");

    // Prepend the fake bin directory to PATH so the fake binary is found first.
    let original_path = EnvVars.var_os(EnvVars::PATH).unwrap_or_default();
    let mut new_path = std::ffi::OsString::from(fake_bin_dir.path());
    new_path.push(":");
    new_path.push(&original_path);

    // Run prek with the modified PATH.
    // Before the fix: this would fail with a clap argument parsing error like:
    //   "unexpected argument '/path/to/trailing-whitespace-fixer' found"
    // After the fix: this should pass because builtin hooks use split() not resolve(None).
    cmd_snapshot!(context, context.run().env("PATH", new_path), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - description: Trims trailing whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing test.txt

    ----- stderr -----
    ");

    // Verify the file was fixed (trailing whitespace removed).
    assert_eq!(context.read("test.txt"), "hello world\n");

    Ok(())
}
