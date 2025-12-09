/// Test `language: unsupported` and `language: unsupported_script` works.
#[cfg(unix)]
#[test]
fn unsupported_language() -> anyhow::Result<()> {
    use crate::common::{TestContext, cmd_snapshot};
    use assert_fs::fixture::{FileWriteStr, PathChild};

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
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
    "});
    context
        .work_dir()
        .child("script.sh")
        .write_str(indoc::indoc! {r#"
            #!/usr/bin/env bash
            echo "Hello, World!"
        "#})?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    unsupported..............................................................Passed
    - hook id: unsupported
    - duration: [TIME]

      script.sh .pre-commit-config.yaml
    unsupported-script.......................................................Passed
    - hook id: unsupported-script
    - duration: [TIME]

      Hello, World!

    ----- stderr -----
    ");

    Ok(())
}
