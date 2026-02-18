use anyhow::Result;
use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

#[cfg(unix)]
mod unix {
    use super::*;

    use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
    use prek_consts::PRE_COMMIT_CONFIG_YAML;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn script_run() {
        let context = TestContext::new();
        context.init_project();
        context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/script-hooks
            rev: v1.0.0
            hooks:
              - id: echo-env
                env:
                  VAR2: universe
                verbose: true
              - id: echo-env
                env:
                  VAR1: everyone
                  VAR2: galaxy
                verbose: true
        "});
        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        echo-env.................................................................Passed
        - hook id: echo-env
        - duration: [TIME]

          Hello world and universe!
        echo-env.................................................................Passed
        - hook id: echo-env
        - duration: [TIME]

          Hello everyone and galaxy!

        ----- stderr -----
        ");
    }

    #[test]
    fn workspace_script_run() -> Result<()> {
        let context = TestContext::new();
        context.init_project();

        let config = indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: script
                name: script
                language: script
                entry: ./script.sh
                env:
                  MESSAGE: "Hello, World"
                verbose: true
        "#};
        context.write_pre_commit_config(config);
        context
            .work_dir()
            .child("script.sh")
            .write_str(indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "$MESSAGE!"
        "#})?;

        let child = context.work_dir().child("child");
        child.create_dir_all()?;
        child.child(PRE_COMMIT_CONFIG_YAML).write_str(config)?;
        child.child("script.sh").write_str(indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "$MESSAGE from child!"
        "#})?;

        fs_err::set_permissions(
            context.work_dir().child("script.sh"),
            std::fs::Permissions::from_mode(0o755),
        )?;
        fs_err::set_permissions(
            child.child("script.sh"),
            std::fs::Permissions::from_mode(0o755),
        )?;
        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        Running hooks for `child`:
        script...................................................................Passed
        - hook id: script
        - duration: [TIME]

          Hello, World from child!

        Running hooks for `.`:
        script...................................................................Passed
        - hook id: script
        - duration: [TIME]

          Hello, World!

        ----- stderr -----
        ");

        cmd_snapshot!(context.filters(), context.run().current_dir(&child), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        script...................................................................Passed
        - hook id: script
        - duration: [TIME]

          Hello, World from child!

        ----- stderr -----
        ");

        Ok(())
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

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        echo.....................................................................Passed
        - hook id: echo
        - duration: [TIME]

          Hello, World!

        ----- stderr -----
        ");

        Ok(())
    }

    #[test]
    fn inline_script_no_shebang() {
        let context = TestContext::new();
        context.init_project();
        context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: inline
                name: inline
                language: script
                entry: |
                  echo "inline hello"
                  echo "$MESSAGE"
                env:
                  MESSAGE: world
                pass_filenames: false
                verbose: true
        "#});
        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        inline...................................................................Passed
        - hook id: inline
        - duration: [TIME]

          inline hello
          world

        ----- stderr -----
        ");
    }

    #[test]
    fn inline_script_with_env_s_shebang_and_filenames() -> Result<()> {
        let context = TestContext::new();
        context.init_project();

        // pass_filenames defaults to true, keep it that way to exercise "$@".
        context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: inline
                name: inline
                language: script
                files: ^a\.txt$
                entry: |
                  #!/usr/bin/env -S bash -e
                  printf 'args:'
                  printf ' %s' "$@"
                  printf '\n'
                verbose: true
        "#});

        // Ensure there's at least one filename.
        context.work_dir().child("a.txt").write_str("a")?;
        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        inline...................................................................Passed
        - hook id: inline
        - duration: [TIME]

          args: a.txt

        ----- stderr -----
        ");

        Ok(())
    }

    #[test]
    fn inline_script_with_sh_shebang() {
        let context = TestContext::new();
        context.init_project();
        context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: inline
                name: inline
                language: script
                entry: |
                  #!/usr/bin/env sh
                  echo "hello from sh"
                pass_filenames: false
                verbose: true
        "#});
        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        inline...................................................................Passed
        - hook id: inline
        - duration: [TIME]

          hello from sh

        ----- stderr -----
        ");
    }

    #[test]
    fn inline_script_no_shebang_crlf_line_endings() {
        let context = TestContext::new();
        context.init_project();

        // Ensure CRLF is accepted (common on Windows checkouts) even on unix.
        let config = indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: inline
                name: inline
                language: script
                entry: "echo one\r\necho two\r\n"
                pass_filenames: false
                verbose: true
        "#}
        .replace('\n', "\r\n");

        context.write_pre_commit_config(&config);
        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        inline...................................................................Passed
        - hook id: inline
        - duration: [TIME]

          one
          two

        ----- stderr -----
        ");
    }

    #[test]
    fn inline_script_no_shebang_stops_on_error() {
        let context = TestContext::new();
        context.init_project();
        context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: inline
                name: inline
                language: script
                entry: |
                  false
                  echo "should not print"
                pass_filenames: false
                verbose: true
        "#});
        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: false
        exit_code: 1
        ----- stdout -----
        inline...................................................................Failed
        - hook id: inline
        - duration: [TIME]
        - exit code: 1

        ----- stderr -----
        ");
    }

    #[test]
    fn multiline_entry_still_script_path() -> Result<()> {
        let context = TestContext::new();
        context.init_project();

        // Entry is written as a YAML block scalar (contains newlines) but the first token is a
        // real script path, so it should be treated as a normal `script` hook.
        context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: script
                name: script
                language: script
                entry: |
                  ./script.sh --from-entry
                pass_filenames: false
                verbose: true
        "});

        let script = context.work_dir().child("script.sh");
        script.write_str(indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "ran script.sh $1"
        "#})?;
        fs_err::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;

        context.git_add(".");

        cmd_snapshot!(context.filters(), context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        script...................................................................Passed
        - hook id: script
        - duration: [TIME]

          ran script.sh --from-entry

        ----- stderr -----
        ");

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

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]

      Hello, World!

    ----- stderr -----
    ");

    Ok(())
}
