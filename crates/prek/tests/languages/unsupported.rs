/// Test `language: unsupported` and `language: unsupported_script` works.
#[cfg(unix)]
#[test]
fn unsupported_language() {
    use crate::common::{TestEnv, cmd_snapshot};

    let context = TestEnv::new_git()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: unsupported
                name: unsupported
                language: unsupported
                entry: echo
                verbose: true
              - id: unsupported-script
                name: unsupported-script
                language: unsupported_script
                entry: ./script.sh
                verbose: true
    "})
        .with_file(
            "script.sh",
            indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "Hello, World!"
        "#},
        );

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    unsupported..............................................................Passed
    - hook id: unsupported
    - duration: [TIME]

      .pre-commit-config.yaml script.sh
    unsupported-script.......................................................Passed
    - hook id: unsupported-script
    - duration: [TIME]

      Hello, World!

    ----- stderr -----
    "#);
}
