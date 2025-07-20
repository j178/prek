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

    // Set up the config to use the built-in end-of-file-fixer
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: end-of-file-fixer
    "#});

    let cwd = context.workdir();

    // A file that is already correct (ends with one newline)
    cwd.child("correct.txt").write_str("Hello World\n")?;
    // A file with no trailing newline
    cwd.child("no_newline.txt")
        .write_str("No trailing newline")?;
    // A file with multiple trailing newlines
    cwd.child("multiple_newlines.txt")
        .write_str("Multiple newlines\n\n\n")?;
    // An empty file
    cwd.child("empty.txt").touch()?;
    // A file containing only newlines
    cwd.child("only_newlines.txt").write_str("\n\n")?;
    // Stage all files
    context.git_add(".");

    // First run: hooks should fail and fix the files
    cmd_snapshot!(context.filters(), context.run(), @r###"
    success: false
    exit_code: 1
    ----- stdout -----
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - exit code: 1
    - files were modified by this hook
      Fixing no_newline.txt
      Fixing multiple_newlines.txt
      Fixing only_newlines.txt

    ----- stderr -----
    "###);

    // Assert that the files have been corrected
    assert_snapshot!(context.read("correct.txt"), @"Hello World\n");
    assert_snapshot!(context.read("no_newline.txt"), @"No trailing newline\n");
    assert_snapshot!(context.read("multiple_newlines.txt"), @"Multiple newlines\n");
    assert_snapshot!(context.read("empty.txt"), @"");
    assert_snapshot!(context.read("only_newlines.txt"), @"\n");

    // Stage the fixes
    context.git_add(".");

    // Second run: hooks should now pass
    cmd_snapshot!(context.filters(), context.run(), @r###"
    success: true
    exit_code: 0
    ----- stdout -----
    fix end of files.........................................................Passed

    ----- stderr -----
    "###);

    Ok(())
}
