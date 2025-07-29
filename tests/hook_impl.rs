use std::process::Command;

use common::TestContext;
use indoc::indoc;

use crate::common::cmd_snapshot;

mod common;

#[test]
fn hook_impl() {
    let context = TestContext::new();

    context.init_project();

    context.write_pre_commit_config(indoc! { r"
        repos:
        - repo: local
          hooks:
           - id: fail
             name: fail
             language: fail
             entry: always fail
             always_run: true
    "});

    context.git_add(".");
    context.configure_git_author();
    let mut commit = Command::new("git");
    commit
        .arg("commit")
        .current_dir(context.work_dir())
        .arg("-m")
        .arg("Initial commit");

    cmd_snapshot!(context.filters(), context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    prefligit installed at .git/hooks/pre-commit

    ----- stderr -----
    "#);

    cmd_snapshot!(context.filters(), commit, @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1
      always fail

      .pre-commit-config.yaml
    "#);
}
