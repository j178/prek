mod common;

use anyhow::Result;
use indoc::indoc;

use crate::common::{TestContext, cmd_snapshot};

#[test]
fn basic_discovery() -> Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();

    let config = indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: show-cwd
          name: Show CWD
          language: python
          entry: python -c 'import sys, os; print(os.getcwd()); print(sys.argv[1:])'
          verbose: true
    "};

    context.setup_workspace(
        &[
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    )?;
    context.git_add(".");

    // Run from the root directory
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `nested/project4`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/nested/project4
      ['.pre-commit-config.yaml']

    Running hooks for `project3/project5`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3/project5
      ['.pre-commit-config.yaml']

    Running hooks for `project2`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project2
      ['.pre-commit-config.yaml']

    Running hooks for `project3`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['project5/.pre-commit-config.yaml', '.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/
      ['nested/project4/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project5/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
      [TEMP_DIR]/
      ['project3/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // Run from a subdirectory
    cmd_snapshot!(context.filters(), context.run().current_dir(cwd.join("project2")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project2
      ['.pre-commit-config.yaml']

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().current_dir(cwd.join("project2")).arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project2
      ['.pre-commit-config.yaml']

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().current_dir(cwd.join("project3")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project5`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3/project5
      ['.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['project5/.pre-commit-config.yaml', '.pre-commit-config.yaml']

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--cd").arg(cwd.join("project3")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project5`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3/project5
      ['.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['project5/.pre-commit-config.yaml', '.pre-commit-config.yaml']

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn config_not_staged() -> Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();

    let config = indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: show-cwd
          name: Show CWD
          language: python
          entry: python -c 'import sys, os; print(os.getcwd()); print(sys.argv[1:])'
          verbose: true
    "};
    context.setup_workspace(
        &[
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    )?;
    context.git_add(".");

    let config = indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: show-cwd-modified
          name: Show CWD
          language: python
          entry: python -c 'import sys, os; print(os.getcwd()); print(sys.argv[1:])'
          verbose: true
    "};
    // Setup again to modify files after git add
    context.setup_workspace(
        &[
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    )?;

    // Run from the root directory
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: The following configuration files are not staged, `git add` them first:
      .pre-commit-config.yaml
      nested/project4/.pre-commit-config.yaml
      project2/.pre-commit-config.yaml
      project3/.pre-commit-config.yaml
      project3/project5/.pre-commit-config.yaml
    ");

    // Run from a subdirectory
    cmd_snapshot!(context.filters(), context.run().current_dir(cwd.join("project3")), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: The following configuration files are not staged, `git add` them first:
      .pre-commit-config.yaml
      project5/.pre-commit-config.yaml
    ");

    cmd_snapshot!(context.filters(), context.run().current_dir(cwd.join("project2")), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: prek configuration file is not staged, run `git add .pre-commit-config.yaml` to stage it
    ");

    Ok(())
}
