use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestEnv, cmd_snapshot};

/// GitHub Action only has docker for linux hosted runners.
#[test]
fn docker() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/prek-ci/docker-hooks
            rev: v1.0
            hooks:
              - id: hello-world
                entry: "sh -c 'echo $MESSAGE! $*' --"
                env:
                    MESSAGE: "Hello, world"
                verbose: true
                always_run: true
    "#});

    context.git_add_all();

    cmd_snapshot!(context, context.run(), @r#"
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
    let context = TestEnv::new();
    let cwd = context.work_dir();

    let config = indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-ci/docker-hooks
            rev: v1.0
            hooks:
              - id: hello-world
                entry: echo
                verbose: true
    "};

    context.setup_workspace(&["project1", "project2"], config)?;
    cwd.child("project1").child("project1.txt").write_str("")?;
    cwd.child("project2").child("project2.txt").write_str("")?;

    context.git_add_all();

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project1
      Hello World............................................................Passed
      - hook id: hello-world
      - duration: [TIME]

        .pre-commit-config.yaml project1.txt
    ✓ project2
      Hello World............................................................Passed
      - hook id: hello-world
      - duration: [TIME]

        .pre-commit-config.yaml project2.txt
    ✓ <workspace>
      Hello World............................................................Passed
      - hook id: hello-world
      - duration: [TIME]

        .pre-commit-config.yaml project1/.pre-commit-config.yaml project2/.pre-commit-config.yaml project1/project1.txt
        project2/project2.txt

    ----- stderr -----
    "#);

    Ok(())
}
