use crate::common::{TestContext, cmd_snapshot};

#[test]
fn unimplemented_language() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
            - id: unimplemented-language-hook
              name: r-hook
              language: r
              entry: rscript --version
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    r-hook...............................................(unimplemented yet)Skipped

    ----- stderr -----
    warning: Some hooks were skipped because their languages are unimplemented.
    We're working hard to support more languages. Check out current support status at https://prek.j178.dev/todo/#language-support-status.
    ");
}
