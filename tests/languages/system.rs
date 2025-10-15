use crate::common::{TestContext, cmd_snapshot};

#[test]
fn system_language_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        default_language_version:
          node: system
        repos:
          - repo: local
            hooks:
              - id: node-version
                name: node-version
                language: node
                entry: node --version
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    node-version.............................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn system_hooks() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: echo
                name: echo
                language: system
                entry: echo hello world
                always_run: true
                pass_filenames: false
                verbose: true
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]
      hello world

    ----- stderr -----
    "#);
}
