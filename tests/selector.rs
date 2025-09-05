mod common;

use indoc::indoc;

use crate::common::{TestContext, cmd_snapshot};

#[test]
fn selector_hook_ids() {
    let context = TestContext::new();
    context.init_project();

    let config = indoc! {r#"
    repos:
      - repo: local
        hooks:
        - id: black
          name: Black
          language: system
          entry: echo "black ran"
    "#};

    context.write_pre_commit_config(config);
    context.git_add(".");

    // Test selecting specific hook by ID
    cmd_snapshot!(context.filters(), context.run().arg("black"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    warning: selector `black` (normalized to `:black`) did not match any hooks or projects
    error: No hooks found after filtering with the given selectors
    ");
}
