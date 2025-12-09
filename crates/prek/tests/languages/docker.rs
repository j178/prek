use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

/// GitHub Action only has docker for linux hosted runners.
#[test]
fn docker() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-test-repos/docker-hooks
            rev: v1.0
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

#[test]
fn workspace_docker() -> anyhow::Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();

    let config = indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/docker-hooks
            rev: v1.0
            hooks:
              - id: hello-world
                entry: echo
                verbose: true
    "};

    context.setup_workspace(&["project1", "project2"], config)?;
    cwd.child("project1").child("project1.txt").write_str("")?;
    cwd.child("project2").child("project2.txt").write_str("")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project1`:
    Hello World..............................................................Passed
    - hook id: hello-world
    - duration: [TIME]

      project1.txt .pre-commit-config.yaml

    Running hooks for `project2`:
    Hello World..............................................................Passed
    - hook id: hello-world
    - duration: [TIME]

      project2.txt .pre-commit-config.yaml

    Running hooks for `.`:
    Hello World..............................................................Passed
    - hook id: hello-world
    - duration: [TIME]

      project1/.pre-commit-config.yaml .pre-commit-config.yaml project2/project2.txt project1/project1.txt
      project2/.pre-commit-config.yaml

    ----- stderr -----
    ");

    Ok(())
}
