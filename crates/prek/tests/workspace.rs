mod common;

use std::process::Command;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteStr, PathChild};
use indoc::indoc;
use prek_consts::env_vars::EnvVars;

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

    // Add an invalid config
    context
        .work_dir()
        .child("project3/.pre-commit-config.yaml")
        .write_str("invalid_yaml: [")?;
    context.git_add(".");

    // Should error out because of the invalid config
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `project3/.pre-commit-config.yaml`
      caused by: did not find expected node content at line 2 column 1, while parsing a flow node
    ");

    // Should skip the invalid config
    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("project3/"), @r"
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
      ['project2/.pre-commit-config.yaml', '.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn workspace_no_projects() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config("repos: []");
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().arg("--skip").arg("."), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: No `prek.toml` or `.pre-commit-config.yaml` found in the current directory or parent directories.

    hint: If you just added one, rerun your command with the `--refresh` flag to rescan the workspace.
    ");
}

#[test]
fn gitignore_respected() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let config = indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: show-cwd
          name: Show CWD
          language: python
          entry: python -c 'import sys, os; print(os.getcwd()); print(sorted(sys.argv[1:]))'
          verbose: true
    "};

    // Create a project structure with directories that should be ignored
    context.setup_workspace(
        &[
            "src",
            "node_modules/ignored", // Should be ignored by .gitignore
            "target/ignored",       // Should be ignored by .gitignore
        ],
        config,
    )?;

    // Create .gitignore that ignores node_modules and target
    context
        .work_dir()
        .child(".gitignore")
        .write_str("node_modules/\ntarget/\n")?;

    context.git_add(".");

    // Run from the root - should not discover projects in node_modules or target
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `src`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]

      [TEMP_DIR]/src
      ['.pre-commit-config.yaml']

    Running hooks for `.`:
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]

      [TEMP_DIR]/
      ['.gitignore', '.pre-commit-config.yaml', 'src/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    Ok(())
}

/// Tests that `--files` arguments references files in other projects, should be filtered out properly.
#[test]
fn reference_files_across_projects() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let config = indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: echo
          name: echo
          language: system
          entry: echo
          verbose: true
    "};

    // Create a project structure with directories that should be ignored
    context.setup_workspace(&["frontend", "backend"], config)?;

    let cwd = context.work_dir();
    cwd.child("backend/app.py")
        .write_str("print('Hello from backend')")?;
    context.git_add(".");
    // Run with --files referencing a file in another project
    cmd_snapshot!(context.filters(), context.run().current_dir(cwd.child("frontend")).arg("--files").arg("../backend/app.py").arg("../backend/non-exist.py"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.................................................(no files to check)Skipped

    ----- stderr -----
    warning: This file does not exist and will be ignored: `../backend/non-exist.py`
    ");

    Ok(())
}

#[test]
fn submodule_discovery() -> Result<()> {
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

    context.setup_workspace(&["project2"], config)?;

    // Create a submodule
    let submodule_path = cwd.child("submodule");
    let submodule_context = TestContext::new_at(submodule_path.to_path_buf());

    submodule_context.init_project();
    submodule_context.configure_git_author();
    submodule_context.write_pre_commit_config(config);
    submodule_context.git_add(".");
    submodule_context.git_commit("Initial commit");

    // Add submodule to the main project
    Command::new("git")
        .args(["submodule", "add", "./submodule"])
        .current_dir(cwd)
        .assert()
        .success();
    context.git_add(".");

    // 1. Test that workspace discovery does not recurse into git submodules
    cmd_snapshot!(context.filters(), context.run().arg("--all-files"), @r"
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
      ['.pre-commit-config.yaml', '.gitmodules', 'project2/.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // 2. Test that current directory is in the submodule with a .pre-commit-config
    cmd_snapshot!(context.filters(), context.run().current_dir(&submodule_path).arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]

      [TEMP_DIR]/submodule
      ['.pre-commit-config.yaml']

    ----- stderr -----
    ");

    // 3. Test that current directory is in the submodule without .pre-commit-config
    // Remove the config file in the submodule
    std::fs::remove_file(submodule_path.join(".pre-commit-config.yaml"))?;
    submodule_context.git_add(".");
    submodule_context.git_commit("Remove config");

    cmd_snapshot!(context.filters(), context.run().current_dir(&submodule_path), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: No `prek.toml` or `.pre-commit-config.yaml` found in the current directory or parent directories.

    hint: If you just added one, rerun your command with the `--refresh` flag to rescan the workspace.
    ");

    Ok(())
}

#[test]
fn orphan_projects() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Create a hook that shows which files it processes
    let config = indoc! {r#"
    exclude: \.pre-commit-config\.yaml$
    repos:
      - repo: local
        hooks:
        - id: show-files
          name: Show Files
          language: python
          entry: python -c 'import sys; print("Processing {} files".format(len(sys.argv[1:]))); [print("  - {}".format(f)) for f in sys.argv[1:]]'
          pass_filenames: true
          verbose: true
    "#};

    // Setup workspace with nested projects
    context
        .work_dir()
        .child("src/backend/.pre-commit-config.yaml")
        .write_str(config)?;
    context
        .work_dir()
        .child("src/.pre-commit-config.yaml")
        .write_str(config)?;
    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .write_str(config)?;

    // Create test files
    context
        .work_dir()
        .child("src/backend/test.py")
        .write_str("")?;
    context.work_dir().child("src/test.py").write_str("")?;
    context.work_dir().child("test.py").write_str("")?;
    context.git_add(".");

    // Without `orphan`: files in subprojects are processed multiple times
    cmd_snapshot!(context.filters(), context.run().arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `src/backend`:
    Show Files...............................................................Passed
    - hook id: show-files
    - duration: [TIME]

      Processing 1 files
        - test.py

    Running hooks for `src`:
    Show Files...............................................................Passed
    - hook id: show-files
    - duration: [TIME]

      Processing 2 files
        - test.py
        - backend/test.py

    Running hooks for `.`:
    Show Files...............................................................Passed
    - hook id: show-files
    - duration: [TIME]

      Processing 3 files
        - src/test.py
        - src/backend/test.py
        - test.py

    ----- stderr -----
    ");

    // Enable `orphan`
    context
        .work_dir()
        .child("src/backend/.pre-commit-config.yaml")
        .write_str(indoc! {r#"
        orphan: true
        exclude: \.pre-commit-config\.yaml$
        repos:
          - repo: local
            hooks:
            - id: show-files
              name: Show Files
              language: python
              entry: python -c 'import sys; print("Processing {} files".format(len(sys.argv[1:]))); [print("  - {}".format(f)) for f in sys.argv[1:]]'
              pass_filenames: true
              verbose: true
    "#})?;

    // `files` match nothing, but files are still "consumed"
    context
        .work_dir()
        .child("src/.pre-commit-config.yaml")
        .write_str(indoc! {r#"
        orphan: true
        files: ^$
        exclude: \.pre-commit-config\.yaml$
        repos:
          - repo: local
            hooks:
            - id: show-files
              name: Show Files
              language: python
              entry: python -c 'import sys; print("Processing {} files".format(len(sys.argv[1:]))); [print("  - {}".format(f)) for f in sys.argv[1:]]'
              pass_filenames: true
              verbose: true
    "#})?;

    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .write_str(indoc! {r#"
        orphan: false
        exclude: \.pre-commit-config\.yaml$
        repos:
          - repo: local
            hooks:
            - id: show-files
              name: Show Files
              language: python
              entry: python -c 'import sys; print("Processing {} files".format(len(sys.argv[1:]))); [print("  - {}".format(f)) for f in sys.argv[1:]]'
              pass_filenames: true
              verbose: true
    "#})?;

    // In orphan project, files are "consumed" and not processed again in parent projects
    cmd_snapshot!(context.filters(), context.run().arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `src/backend`:
    Show Files...............................................................Passed
    - hook id: show-files
    - duration: [TIME]

      Processing 1 files
        - test.py

    Running hooks for `src`:
    Show Files...........................................(no files to check)Skipped

    Running hooks for `.`:
    Show Files...............................................................Passed
    - hook id: show-files
    - duration: [TIME]

      Processing 1 files
        - test.py

    ----- stderr -----
    ");

    // If hooks in orphan projects are not selected, files should be "consumed" as well
    cmd_snapshot!(context.filters(), context.run().arg("--all-files").arg("--skip").arg("src/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show Files...............................................................Passed
    - hook id: show-files
    - duration: [TIME]

      Processing 1 files
        - test.py

    ----- stderr -----
    ");

    Ok(())
}
