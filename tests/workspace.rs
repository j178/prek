mod common;

use anyhow::Result;
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
    error: No `.pre-commit-config.yaml` found in the current directory or parent directories.

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

#[test]
fn relative_repo_path_resolution() -> Result<()> {
    use std::process::Command;
    use assert_fs::fixture::{PathChild, FileWriteStr, PathCreateDir};
    use assert_cmd::assert::OutputAssertExt;
    use prek_consts::MANIFEST_FILE;
    
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    // Create a local hook repository at the root level
    let hook_repo = context.work_dir().child("hook-repo");
    hook_repo.create_dir_all()?;

    Command::new("git")
        .arg("init")
        .current_dir(&hook_repo)
        .assert()
        .success();

    Command::new("git")
        .arg("config")
        .arg("user.name")
        .arg("Prek Test")
        .current_dir(&hook_repo)
        .assert()
        .success();
    
    Command::new("git")
        .arg("config")
        .arg("user.email")
        .arg("test@prek.dev")
        .current_dir(&hook_repo)
        .assert()
        .success();
        
    Command::new("git")
        .arg("config")
        .arg("core.autocrlf")
        .arg("false")
        .current_dir(&hook_repo)
        .assert()
        .success();

    hook_repo.child(MANIFEST_FILE).write_str(indoc! {r"
        - id: test-hook
          name: Test Hook  
          entry: echo test
          language: system
    "})?;

    Command::new("git")
        .arg("add")
        .arg(".")
        .current_dir(&hook_repo)
        .assert()
        .success();

    Command::new("git")
        .arg("commit")
        .arg("-m")
        .arg("Initial commit")
        .current_dir(&hook_repo)
        .assert()
        .success();

    // Get the commit SHA
    let output = Command::new("git")
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(&hook_repo)
        .output()?;
    let commit_sha = String::from_utf8_lossy(&output.stdout).trim().to_string();

    // Create a subdirectory project that references the hook repo with a relative path
    let subproject = context.work_dir().child("subproject");
    subproject.create_dir_all()?;

    // Write a config that uses a relative path to the hook repo
    // From subproject/, ../hook-repo should resolve correctly
    subproject.child(".pre-commit-config.yaml").write_str(&format!(
        indoc! {r"
        repos:
          - repo: ../hook-repo
            rev: {}
            hooks:
              - id: test-hook
                always_run: true
    "}, commit_sha))?;

    // Create a test file in the subproject
    subproject.child("test.txt").write_str("test content")?;
    
    // Add a root config as well, otherwise discovery will fail
    context.write_pre_commit_config(indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: noop
                name: Noop
                entry: echo noop
                language: system
                always_run: true
    "});

    context.git_add(".");

    // Run from the root directory - the relative path should resolve from subproject/.pre-commit-config.yaml location
    // The test verifies that ../hook-repo is correctly resolved relative to subproject/, not the CWD
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Running hooks for `subproject`:
    Test Hook................................................................Passed

    Running hooks for `.`:
    Noop.....................................................................Passed

    ----- stderr -----
    ");

    Ok(())
}
