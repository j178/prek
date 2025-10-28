use crate::common::{TestContext, cmd_snapshot};

#[test]
fn unimplemented_language() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
            - id: rust-hook
              name: rust-hook
              language: rust
              entry: cargo run
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    rust-hook............................................(unimplemented yet)Skipped

    ----- stderr -----
    warning: Some hooks were skipped because their languages are unimplemented.
    We're working hard to support more languages. Check out current support status at https://prek.j178.dev/todo/#language-support-status.
    ");
}
