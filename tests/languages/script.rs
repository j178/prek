use crate::common::{TestContext, cmd_snapshot};

#[test]
fn terraform_hooks() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prefligit-test-repos/script-hooks
            rev: main
            hooks:
              - id: echo
                verbose: true
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]
      .pre-commit-config.yaml

    ----- stderr -----
    ");
}
