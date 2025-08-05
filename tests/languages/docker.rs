use crate::common::{TestContext, cmd_snapshot};

/// GitHub Action only has docker for linux hosted runners.
#[test]
fn docker() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prefligit-test-repos/docker-hooks
            rev: master
            hooks:
              - id: hello-world
                entry: "echo Hello, world!"
                verbose: true
                always_run: true
    "#});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Hello World..............................................................Passed
    - hook id: hello-world
    - duration: [TIME]
      Hello, world! .pre-commit-config.yaml

    ----- stderr -----
    "#);
}
