mod common;

use anyhow::Result;
use assert_fs::fixture::{FileWriteStr, PathChild};
use constants::env_vars::EnvVars;
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

    // Ignore `project5` in `project3`
    context
        .work_dir()
        .child("project3/.prekignore")
        .write_str("project5/\n")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("--refresh").arg("--cd").arg(cwd.join("project3")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['.prekignore', '.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // Ignoring everything under project3, but when runs from project3, it’s still getting picked up.
    context
        .work_dir()
        .child("project3/.prekignore")
        .write_str("*\n")?;
    context.git_add(".");
    cmd_snapshot!(context.filters(), context.run().arg("--refresh").arg("--cd").arg(cwd.join("project3")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['.prekignore', '.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']

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

#[test]
fn run_with_selectors() -> Result<()> {
    let context = TestContext::new();
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

    cmd_snapshot!(context.filters(), context.run().arg("project2/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project2`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project2
      ['.pre-commit-config.yaml']

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("project2/"), @r"
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

    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("nested/").arg("--skip").arg("project3/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project2`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project2
      ['.pre-commit-config.yaml']

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

    cmd_snapshot!(context.filters(), context.run().arg("show-cwd"), @r"
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

    cmd_snapshot!(context.filters(), context.run().arg("project2:show-cwd"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project2`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project2
      ['.pre-commit-config.yaml']

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().arg(".:show-cwd"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/
      ['nested/project4/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project5/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
      [TEMP_DIR]/
      ['project3/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("show-cwd"), @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: No hooks found after filtering with the given selectors
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("project2:show-cwd").arg("--skip").arg("nested:show-cwd"), @r"
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
    warning: selector `--skip=nested:show-cwd` did not match any hooks
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("non-exist"), @r"
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
    warning: selector `--skip=non-exist` did not match any hooks
    ");

    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("../"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Invalid selector: `../`
      caused by: Invalid project path: `../`
      caused by: path is outside the workspace root
    ");

    cmd_snapshot!(context.filters(), context.run().current_dir(context.work_dir().join("project2")), @r"
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

    Ok(())
}

#[test]
fn skips() -> Result<()> {
    let context = TestContext::new();
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

    context.setup_workspace(&["project2", "project3", "project3/project4"], config)?;
    context.git_add(".");

    // Test CLI skip
    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("project2/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project3/project4`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3/project4
      ['.pre-commit-config.yaml']

    Running hooks for `project3`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['project4/.pre-commit-config.yaml', '.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/
      ['project2/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // Test PREK_SKIP environment variable
    cmd_snapshot!(context.filters(), context.run().env(EnvVars::PREK_SKIP, "project2/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project3/project4`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3/project4
      ['.pre-commit-config.yaml']

    Running hooks for `project3`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['project4/.pre-commit-config.yaml', '.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/
      ['project2/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // Test SKIP environment variable
    cmd_snapshot!(context.filters(), context.run().env(EnvVars::SKIP, "project2/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project3/project4`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3/project4
      ['.pre-commit-config.yaml']

    Running hooks for `project3`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['project4/.pre-commit-config.yaml', '.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/
      ['project2/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // Test precedence: CLI --skip overrides PREK_SKIP
    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("project2/").env(EnvVars::PREK_SKIP, "project3/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project3/project4`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3/project4
      ['.pre-commit-config.yaml']

    Running hooks for `project3`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['project4/.pre-commit-config.yaml', '.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/
      ['project2/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // Test precedence: PREK_SKIP overrides SKIP
    cmd_snapshot!(context.filters(), context.run().env(EnvVars::PREK_SKIP, "project2/").env(EnvVars::SKIP, "project3/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `project3/project4`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3/project4
      ['.pre-commit-config.yaml']

    Running hooks for `project3`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/project3
      ['project4/.pre-commit-config.yaml', '.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/
      ['project2/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // Test multiple selectors in environment variable
    cmd_snapshot!(context.filters(), context.run().env("PREK_SKIP", "project2/,project3/,non-exist-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]
      [TEMP_DIR]/
      ['project2/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    warning: selector `PREK_SKIP=non-exist-hook` did not match any hooks
    ");

    Ok(())
}
