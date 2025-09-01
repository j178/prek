mod common;

use crate::common::{TestContext, cmd_snapshot};
use anyhow::Result;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use indoc::indoc;

#[test]
fn basic_discovery() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let project1 = context.work_dir();
    let project2 = context.work_dir().child("project2");
    let project3 = context.work_dir().child("project3");
    let project4 = context.work_dir().child("nested/project4");
    let project5 = context.work_dir().child("project3/project5");

    project2.create_dir_all()?;
    project3.create_dir_all()?;
    project4.create_dir_all()?;
    project5.create_dir_all()?;

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

    project1
        .child(".pre-commit-config.yaml")
        .write_str(config)?;
    project2
        .child(".pre-commit-config.yaml")
        .write_str(config)?;
    project3
        .child(".pre-commit-config.yaml")
        .write_str(config)?;
    project4
        .child(".pre-commit-config.yaml")
        .write_str(config)?;
    project5
        .child(".pre-commit-config.yaml")
        .write_str(config)?;

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
    cmd_snapshot!(context.filters(), context.run().current_dir(&project2), @r"
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

    cmd_snapshot!(context.filters(), context.run().current_dir(&project2).arg("--all-files"), @r"
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

    cmd_snapshot!(context.filters(), context.run().current_dir(&project3), @r"
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

    cmd_snapshot!(context.filters(), context.run().arg("--cd").arg(&*project3), @r"
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
