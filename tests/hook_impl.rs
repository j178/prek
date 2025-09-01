use std::io::Write;
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
fn hook_impl_pre_push() {
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

    // Test pre-push hook with stdin input
    let mut hook_cmd = context.command();
    hook_cmd
        .arg("hook-impl")
        .arg("--hook-type")
        .arg("pre-push")
        .arg("--hook-dir")
        .arg(context.work_dir().join(".git/hooks"))
        .arg("--")
        .arg("origin") // remote name
        .arg("https://github.com/test/repo.git"); // remote URL

    // Simulate pre-push stdin: local_ref local_sha remote_ref remote_sha
    let stdin_input = "refs/heads/main abc123def456789012345678901234567890abc refs/heads/main 0000000000000000000000000000000000000000\n";

    hook_cmd.stdin(std::process::Stdio::piped());
    let mut child = hook_cmd.spawn().unwrap();
    child
        .stdin
        .as_mut()
        .unwrap()
        .write_all(stdin_input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();

    // For now, just check that the command runs (we'll adjust the snapshot later)
    assert!(!output.status.success());
}
