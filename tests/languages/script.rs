use anyhow::Result;
use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

#[cfg(unix)]
mod unix {
    use super::*;

    use assert_fs::fixture::{FileWriteStr, PathChild};
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn script_run() {
        let context = TestContext::new();
        context.init_project();
        context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/script-hooks
            rev: main
            hooks:
              - id: echo
                verbose: true
        "});
        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r##"
        success: true
        exit_code: 0
        ----- stdout -----
        echo.....................................................................Passed
        - hook id: echo
        - duration: [TIME]
          .pre-commit-config.yaml

        ----- stderr -----
        warning: The following repos have mutable `rev` fields (moving tag / branch):
        https://github.com/prek-test-repos/script-hooks: main
        Mutable references are never updated after first install and are not supported.
        See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.
        Hint: `prek autoupdate` often fixes this",
        "##);
    }

    #[test]
    fn local_repo_bash_shebang() -> Result<()> {
        let context = TestContext::new();
        context.init_project();
        context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: echo
                name: echo
                language: script
                entry: ./echo.sh
                verbose: true
        "});

        let script = context.work_dir().child("echo.sh");
        script.write_str(indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "Hello, World!"
        "#})?;
        fs_err::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;

        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        echo.....................................................................Passed
        - hook id: echo
        - duration: [TIME]
          Hello, World!

        ----- stderr -----
        "#);

        Ok(())
    }
}

/// Test that a script with a shebang line works correctly on Windows.
/// The interpreter must exist in the PATH, the script is not needed to be executable.
#[test]
fn windows_script_run() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: echo
                name: echo
                language: script
                entry: ./echo.sh
                verbose: true
        "});

    let script = context.work_dir().child("echo.sh");
    script.write_str(indoc::indoc! {r#"
            #!/usr/bin/env python3
            print("Hello, World!")
        "#})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        echo.....................................................................Passed
        - hook id: echo
        - duration: [TIME]
          Hello, World!

        ----- stderr -----
        "#);

    Ok(())
}
