use crate::common::{cmd_snapshot, TestContext};
use assert_fs::fixture::{FileWriteStr, PathChild};

mod common;

#[test]
fn clean() -> anyhow::Result<()> {
    let context = TestContext::new();

    context.init_project();
    context
        .workdir()
        .child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r"
            repos:
              - repo: https://github.com/pre-commit/pre-commit-hooks
                rev: v5.0.0
                hooks:
                  - id: trailing-whitespace
                  - id: end-of-file-fixer
                  - id: check-json
        "})?;

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Cloning https://github.com/pre-commit/pre-commit-hooks@v5.0.0
    Installing environment for https://github.com/pre-commit/pre-commit-hooks@v5.0.0
    trim trailing whitespace.............................(no files to check)Skipped
    fix end of files.....................................(no files to check)Skipped
    check json...........................................(no files to check)Skipped

    ----- stderr -----
    "#);
    cmd_snapshot!(context.filters(), context.clean(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Cleaned `[HOME]/`

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Cloning https://github.com/pre-commit/pre-commit-hooks@v5.0.0
    Installing environment for https://github.com/pre-commit/pre-commit-hooks@v5.0.0
    trim trailing whitespace.............................(no files to check)Skipped
    fix end of files.....................................(no files to check)Skipped
    check json...........................................(no files to check)Skipped

    ----- stderr -----
    "#);

    Ok(())
}
