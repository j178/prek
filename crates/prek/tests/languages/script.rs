use crate::common::{TestEnv, cmd_snapshot};

#[cfg(unix)]
mod unix {
    use super::*;

    #[test]
    fn script_run() {
        let context = TestEnv::new()
            .with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-ci/script-hooks
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
        "})
            .init_git();

        cmd_snapshot!(context, context.run(), @r"
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
    fn workspace_script_run() {
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
        let context = TestEnv::new()
            .with_config(config)
            .with_executable_file(
                "script.sh",
                indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "$MESSAGE!"
        "#},
            )
            .with_project_config("child", config)
            .with_executable_file(
                "child/script.sh",
                indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "$MESSAGE from child!"
        "#},
            )
            .init_git();
        let child = context.child("child");

        context.git().add(".");

        cmd_snapshot!(context, context.run(), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        ✓ child
          script.................................................................Passed
          - hook id: script
          - duration: [TIME]

            Hello, World from child!
        ✓ <workspace>
          script.................................................................Passed
          - hook id: script
          - duration: [TIME]

            Hello, World!

        ----- stderr -----
        "#);

        cmd_snapshot!(context, context.run().current_dir(&child), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        script...................................................................Passed
        - hook id: script
        - duration: [TIME]

          Hello, World from child!

        ----- stderr -----
        ");
    }

    #[test]
    fn local_repo_bash_shebang() {
        let context = TestEnv::new()
            .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: echo
                name: echo
                language: script
                entry: ./echo.sh
                verbose: true
        "})
            .with_executable_file(
                "echo.sh",
                indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "Hello, World!"
        "#},
            )
            .init_git();

        cmd_snapshot!(context, context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        echo.....................................................................Passed
        - hook id: echo
        - duration: [TIME]

          Hello, World!

        ----- stderr -----
        ");
    }

    #[test]
    fn script_shell_runs_entry_as_shell_source() {
        let context = TestEnv::new()
            .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: shell-script
                name: shell-script
                language: script
                files: ^a\.txt$
                entry: |
                  printf 'args:'
                  for value in "$@"; do
                    printf ' <%s>' "$value"
                  done
                  printf '\n'
                shell: sh
                args: [configured]
                verbose: true
        "#})
            .with_file("a.txt", "a")
            .init_git();

        cmd_snapshot!(context, context.run(), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        shell-script.............................................................Passed
        - hook id: shell-script
        - duration: [TIME]

          args: <configured> <a.txt>

        ----- stderr -----
        ");
    }
}

/// Test that a script with a shebang line works correctly on Windows.
/// The interpreter must exist in the PATH, the script is not needed to be executable.
#[test]
fn windows_script_run() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
    repos:
      - repo: local
        hooks:
          - id: echo
            name: echo
            language: script
            entry: ./echo.sh
            verbose: true
    "})
        .with_executable_file(
            "echo.sh",
            indoc::indoc! {r#"
        #!/usr/bin/env python3
        print("Hello, World!")
    "#},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]

      Hello, World!

    ----- stderr -----
    ");
}
