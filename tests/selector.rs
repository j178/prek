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
          pass_filenames: false
    "#};

    context.write_pre_commit_config(config);
    context.git_add(".");

    // Test selecting specific hook by ID
    cmd_snapshot!(context.filters(), context.run().arg("black").arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Black....................................................................Passed
    - hook id: black
    - duration: [TIME]
      black ran

    ----- stderr -----
    ");
}
