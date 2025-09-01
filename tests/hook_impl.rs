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
    prek installed at `.git/hooks/pre-commit`

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

#[test]
fn hook_impl_pre_push() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc! { r#"
        repos:
        - repo: local
          hooks:
           - id: success
             name: success
             language: system
             entry: echo "hook ran successfully"
             always_run: true
    "#});

    context.git_add(".");
    context.configure_git_author();

    let mut commit = Command::new("git");
    commit
        .arg("commit")
        .current_dir(context.work_dir())
        .arg("-m")
        .arg("Initial commit");

    cmd_snapshot!(context.filters(), context.install().arg("--hook-type").arg("pre-push"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-push`

    ----- stderr -----
    "#);

    let mut filters = context.filters();
    filters.push((r"\b[0-9a-f]{7}\b", "[SHA1]"));
    cmd_snapshot!(filters, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) [SHA1]] Initial commit
     1 file changed, 8 insertions(+)
     create mode 100644 .pre-commit-config.yaml

    ----- stderr -----
    ");

    // Set up a bare remote repository
    let remote_repo_path = context.home_dir().join("remote.git");
    std::fs::create_dir_all(&remote_repo_path)?;

    let mut init_remote = Command::new("git");
    init_remote
        .arg("init")
        .arg("--bare")
        .arg("--initial-branch=master")
        .current_dir(&remote_repo_path);
    cmd_snapshot!(context.filters(), init_remote, @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Initialized empty Git repository in [HOME]/remote.git/

    ----- stderr -----
    "#);

    // Add remote to local repo
    let mut add_remote = Command::new("git");
    add_remote
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(&remote_repo_path)
        .current_dir(context.work_dir());
    cmd_snapshot!(context.filters(), add_remote, @r#"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    "#);

    // First push - should trigger the hook
    let mut push_cmd = Command::new("git");
    push_cmd
        .arg("push")
        .arg("origin")
        .arg("master")
        .current_dir(context.work_dir());

    cmd_snapshot!(context.filters(), push_cmd, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    success..................................................................Passed

    ----- stderr -----
    To [HOME]/remote.git
     * [new branch]      master -> master
    ");

    // Second push - should not trigger the hook (nothing new to push)
    let mut push_cmd2 = Command::new("git");
    push_cmd2
        .arg("push")
        .arg("origin")
        .arg("master")
        .current_dir(context.work_dir());

    cmd_snapshot!(context.filters(), push_cmd2, @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Everything up-to-date
    ");

    Ok(())
}
