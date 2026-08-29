mod common;

use anyhow::Result;
use indoc::indoc;
use prek_consts::PRE_COMMIT_CONFIG_YAML;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn basic_discovery() {
    let context = TestEnv::new_git();
    let cwd = context.work_dir();
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

    context.write_workspace(
        [
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    );
    context.git().add_all();

    // Run from the root directory
    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ nested/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/nested/project4
        ['.pre-commit-config.yaml']
    ✓ project3/project5
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project5
        ['.pre-commit-config.yaml']
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'nested/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
        [TEMP_DIR]/
        ['project3/project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // Run from a subdirectory
    cmd_snapshot!(context, context.run().current_dir(cwd.join("project2")), @r"
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

    cmd_snapshot!(context, context.run().current_dir(cwd.join("project2")).arg("--all-files"), @r"
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

    cmd_snapshot!(context, context.run().current_dir(cwd.join("project3")), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project5
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project5
        ['.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--cd").arg(cwd.join("project3")), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project5
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project5
        ['.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // Ignore `project5` in `project3`
    context.write_file("project3/.prekignore", "project5/\n");
    context.git().add_all();

    cmd_snapshot!(context, context.run().arg("--refresh").arg("--cd").arg(cwd.join("project3")), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]

      [TEMP_DIR]/project3
      ['.pre-commit-config.yaml', '.prekignore', 'project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // Ignoring everything under project3, but when runs from project3, it’s still getting picked up.
    context.write_file("project3/.prekignore", "*\n");
    context.git().add_all();
    cmd_snapshot!(context, context.run().arg("--refresh").arg("--cd").arg(cwd.join("project3")), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]

      [TEMP_DIR]/project3
      ['.pre-commit-config.yaml', '.prekignore', 'project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);
}

#[test]
fn same_depth_project_concurrency_has_stable_output() {
    let context = TestEnv::new_git().with_config("repos: []").with_file(
        "concurrent_hook.py",
        indoc! {r#"
            from pathlib import Path
            import time

            project = Path.cwd().name
            workspace = Path.cwd().parent
            ready = workspace / f"{project}.ready"
            peer = workspace / f"{'b' if project == 'a' else 'a'}.ready"

            ready.touch()
            deadline = time.monotonic() + 5
            while not peer.exists():
                if time.monotonic() >= deadline:
                    raise TimeoutError(f"timed out waiting for {peer.name}")
                time.sleep(0.01)

            # Make b finish first so output order cannot follow completion order.
            if project == "a":
                time.sleep(0.5)
        "#},
    );

    let config = indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: concurrent-hook
          name: Concurrent Hook
          language: system
          entry: python3 ../concurrent_hook.py
          always_run: true
          pass_filenames: false
    "};

    for project in ["a", "b"] {
        context.write_file(format!("{project}/{PRE_COMMIT_CONFIG_YAML}"), config);
        context.write_file(format!("{project}/file.txt"), "");
    }
    context.git().add_all();

    let mut run = context.run();
    run.arg("--all-files")
        .env(EnvVars::PREK_CONCURRENT_HOOKS, "2");
    cmd_snapshot!(context, run, @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ a
      Concurrent Hook........................................................Passed
    ✓ b
      Concurrent Hook........................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn fail_fast_stops_after_current_project_level() {
    let root_config = indoc::indoc! {r#"
    repos:
      - repo: local
        hooks:
        - id: root-hook
          name: Root Hook
          language: system
          entry: python3 -c "print('root ran')"
          always_run: true
    "#};

    let failing_config = indoc! {r#"
    repos:
      - repo: local
        hooks:
        - id: failing-hook
          name: Failing Hook
          language: system
          entry: python3 -c "import sys; sys.exit(1)"
          always_run: true
          fail_fast: true
    "#};
    let passing_config = indoc! {r#"
    repos:
      - repo: local
        hooks:
        - id: passing-hook
          name: Passing Hook
          language: system
          entry: python3 -c "print('sibling ran')"
          always_run: true
    "#};

    let context = TestEnv::new_git()
        .with_config(root_config)
        .with_file("a/.pre-commit-config.yaml", failing_config)
        .with_file("b/.pre-commit-config.yaml", passing_config);

    context.git().add_all();

    let mut run = context.run();
    run.arg("--all-files")
        .env(EnvVars::PREK_CONCURRENT_HOOKS, "2");
    cmd_snapshot!(context, run, @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    × a
      Failing Hook...........................................................Failed
      - hook id: failing-hook
      - exit code: 1
    ✓ b
      Passing Hook...........................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn config_not_staged() {
    let context = TestEnv::new_git();
    let cwd = context.work_dir();

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
    context.write_workspace(
        [
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    );
    context.git().add_all();

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
    context.write_workspace(
        [
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    );

    // Run from the root directory
    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: The following configuration files are not staged. Stage them with `git add` and try again:
      - `.pre-commit-config.yaml`
      - `nested/project4/.pre-commit-config.yaml`
      - `project2/.pre-commit-config.yaml`
      - `project3/.pre-commit-config.yaml`
      - `project3/project5/.pre-commit-config.yaml`
    ");

    // Run from a subdirectory
    cmd_snapshot!(context, context.run().current_dir(cwd.join("project3")), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: The following configuration files are not staged. Stage them with `git add` and try again:
      - `.pre-commit-config.yaml`
      - `project5/.pre-commit-config.yaml`
    ");

    cmd_snapshot!(context, context.run().current_dir(cwd.join("project2")), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Configuration file `.pre-commit-config.yaml` is not staged. Stage it with `git add` and try again
    ");
}

#[test]
fn run_with_selectors() {
    let context = TestEnv::new_git();

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

    context.write_workspace(
        [
            "project2",
            "project3",
            "nested/project4",
            "project3/project5",
        ],
        config,
    );
    context.git().add_all();

    cmd_snapshot!(context, context.run().arg("--hide-status").arg("passed"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("project2/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--skip").arg("project2/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ nested/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/nested/project4
        ['.pre-commit-config.yaml']
    ✓ project3/project5
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project5
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'nested/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
        [TEMP_DIR]/
        ['project3/project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--skip").arg("nested/").arg("--skip").arg("project3/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'nested/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
        [TEMP_DIR]/
        ['project3/project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("show-cwd"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ nested/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/nested/project4
        ['.pre-commit-config.yaml']
    ✓ project3/project5
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project5
        ['.pre-commit-config.yaml']
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'nested/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
        [TEMP_DIR]/
        ['project3/project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("project2:show-cwd"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg(".:show-cwd"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]

      [TEMP_DIR]/
      ['.pre-commit-config.yaml', 'nested/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
      [TEMP_DIR]/
      ['project3/project5/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--skip").arg("show-cwd"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ nested/project4
      Show CWD..............................................................Skipped
    ✓ project3/project5
      Show CWD..............................................................Skipped
    ✓ project2
      Show CWD..............................................................Skipped
    ✓ project3
      Show CWD..............................................................Skipped
    ✓ <workspace>
      Show CWD..............................................................Skipped

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.run().arg("--skip").arg("project2:show-cwd").arg("--skip").arg("nested:show-cwd"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ nested/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/nested/project4
        ['.pre-commit-config.yaml']
    ✓ project3/project5
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project5
        ['.pre-commit-config.yaml']
    ✓ project2
      Show CWD..............................................................Skipped
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'nested/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
        [TEMP_DIR]/
        ['project3/project5/.pre-commit-config.yaml']

    ----- stderr -----
    warning: selector `--skip=nested:show-cwd` did not match any hooks
    "#);

    cmd_snapshot!(context, context.run().arg("--skip").arg("non-exist"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ nested/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/nested/project4
        ['.pre-commit-config.yaml']
    ✓ project3/project5
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project5
        ['.pre-commit-config.yaml']
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project5/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'nested/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']
        [TEMP_DIR]/
        ['project3/project5/.pre-commit-config.yaml']

    ----- stderr -----
    warning: selector `--skip=non-exist` did not match any hooks
    "#);

    cmd_snapshot!(context, context.run().arg("--skip").arg("../"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Invalid selector: `../`
      caused by: Invalid project path: `../`
      caused by: path is outside the workspace root
    ");

    cmd_snapshot!(context, context.run().current_dir(context.work_dir().join("project2")), @r"
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
}

#[test]
fn run_with_mixed_project_and_hook_selectors() {
    let context = TestEnv::new_git()
        .with_config(indoc::indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: root-hook
          name: root hook
          entry: echo root
          language: system
          pass_filenames: false
    "})
        .with_project_config(
            "sub",
            indoc! {r"
    repos:
      - repo: local
        hooks:
        - id: sub-hook
          name: sub hook
          entry: echo sub
          language: system
          pass_filenames: false
    "},
        )
        .with_file("sub/file.txt", "")
        .with_project_config("empty", "repos: []\n")
        .with_project_config("unselected", "invalid: config\n");

    context.git().add_all();

    cmd_snapshot!(context, context.run().arg("--all-files").arg("sub/").arg(".:root-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ sub
      sub hook...............................................................Passed
    ✓ <workspace>
      root hook..............................................................Passed

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().arg("--all-files").arg("sub/").arg("root-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ sub
      sub hook...............................................................Passed
    ✓ <workspace>
      root hook..............................................................Passed

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.run().arg("--all-files").arg("empty/").arg("root-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    root hook................................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn skips() {
    let context = TestEnv::new_git();

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

    context.write_workspace(["project2", "project3", "project3/project4"], config);
    context.git().add_all();

    // Test CLI skip
    cmd_snapshot!(context, context.run().arg("--skip").arg("project2/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project3/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project4
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project4/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // Test PREK_SKIP environment variable
    cmd_snapshot!(context, context.run().env(EnvVars::PREK_SKIP, "project2/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project3/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project4
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project4/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // Test SKIP environment variable
    cmd_snapshot!(context, context.run().env(EnvVars::SKIP, "project2/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project3/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project4
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project4/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // Test precedence: CLI --skip overrides PREK_SKIP
    cmd_snapshot!(context, context.run().arg("--skip").arg("project2/").env(EnvVars::PREK_SKIP, "project3/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project3/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project4
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project4/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // Test precedence: PREK_SKIP overrides SKIP
    cmd_snapshot!(context, context.run().env(EnvVars::PREK_SKIP, "project2/").env(EnvVars::SKIP, "project3/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project3/project4
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3/project4
        ['.pre-commit-config.yaml']
    ✓ project3
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project3
        ['.pre-commit-config.yaml', 'project4/.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // Test multiple selectors in environment variable
    cmd_snapshot!(context, context.run().env("PREK_SKIP", "project2/,project3/,non-exist-hook"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Show CWD.................................................................Passed
    - hook id: show-cwd
    - duration: [TIME]

      [TEMP_DIR]/
      ['.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    warning: selector `PREK_SKIP=non-exist-hook` did not match any hooks
    ");

    // Add an invalid config
    context.write_file("project3/.pre-commit-config.yaml", "invalid_yaml: [");
    context.git().add_all();

    // Should error out because of the invalid config
    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `project3/.pre-commit-config.yaml`
      caused by: error: line 1 column 15: unclosed bracket '['
     --> <input>:1:15
      |
    1 | invalid_yaml: [
      |               ^ unclosed bracket '['
    ");

    // Should skip the invalid config
    cmd_snapshot!(context, context.run().arg("--skip").arg("project3/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml', 'project3/project4/.pre-commit-config.yaml', 'project3/.pre-commit-config.yaml']

    ----- stderr -----
    "#);
}

#[test]
fn workspace_no_projects() {
    let context = TestEnv::new_git().with_config("repos: []");
    context.git().add_all();

    cmd_snapshot!(context, context.run().arg("--skip").arg("."), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: No `prek.toml` or `.pre-commit-config.yaml` found in the current directory or parent directories.

    hint: If you just added one, rerun your command with the `--refresh` flag to rescan the workspace.
    ");
}

#[test]
fn gitignore_respected() {
    let context = TestEnv::new_git();

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
    context.write_workspace(
        [
            "src",
            "node_modules/ignored", // Should be ignored by .gitignore
            "target/ignored",       // Should be ignored by .gitignore
        ],
        config,
    );

    let context = context.with_file(".gitignore", "node_modules/\ntarget/\n");

    context.git().add_all();

    // Run from the root - should not discover projects in node_modules or target
    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ src
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/src
        ['.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.gitignore', '.pre-commit-config.yaml', 'src/.pre-commit-config.yaml']

    ----- stderr -----
    "#);
}

#[test]
fn nested_project_exclude_is_relative() {
    let context = TestEnv::new_git();

    // Regression test for nested workspaces:
    // `exclude` must be evaluated against paths *relative to each project root*.
    //
    // Concretely:
    // - In the nested project, the file is seen as `excluded_by_project` and should be excluded by `^excluded_by_project$`.
    // - In the root project, the same file is seen as `nested/excluded_by_project` and should NOT be excluded.
    let config = indoc! {r#"
    exclude: \.pre-commit-config\.yaml$|^excluded_by_project$
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

    // Workspace with a nested project.
    context.write_workspace(["nested"], config);

    // A root-level file which should be excluded by the root project (path is `excluded_by_project`).
    // This keeps the snapshot focused on the nested files, while proving the regex is not
    // accidentally matching `nested/excluded_by_project`.
    let context = context.with_files([
        ("excluded_by_project", ""),
        ("nested/include", ""),
        ("nested/excluded_by_project", ""),
    ]);

    context.git().add_all();

    // When running from the root with --all-files, the nested project's exclude
    // pattern should see paths relative to `nested/`, so `noinclude` is excluded
    // there but still visible from the root project.
    cmd_snapshot!(context, context.run().arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ nested
      Show Files.............................................................Passed
      - hook id: show-files
      - duration: [TIME]

        Processing 1 files
          - include
    ✓ <workspace>
      Show Files.............................................................Passed
      - hook id: show-files
      - duration: [TIME]

        Processing 2 files
          - nested/excluded_by_project
          - nested/include

    ----- stderr -----
    "#);
}

/// Tests that `--files` arguments references files in other projects, should be filtered out properly.
#[test]
fn reference_files_across_projects() {
    let context = TestEnv::new_git();

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
    context.write_workspace(["frontend", "backend"], config);

    let context = context.with_file("backend/app.py", "print('Hello from backend')");
    context.git().add_all();
    // Run with --files referencing a file in another project
    cmd_snapshot!(context, context.run().current_dir(context.child("frontend")).arg("--files").arg("../backend/app.py").arg("../backend/non-exist.py"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.................................................(no files to check)Skipped

    ----- stderr -----
    warning: This file does not exist and will be ignored: `../backend/non-exist.py`
    ");
}

#[test]
fn submodule_discovery() -> Result<()> {
    let context = TestEnv::new_git();

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

    context.write_workspace(["project2"], config);

    // Create a submodule
    let submodule_path = context.child("submodule");
    let submodule_context = TestEnv::new_git_at(&submodule_path).with_config(config);
    submodule_context.git().add_all().commit("Initial commit");

    // Add submodule to the main project
    context.git().run(["submodule", "add", "./submodule"]);
    context.git().add_all();

    // 1. Test that workspace discovery does not recurse into git submodules
    cmd_snapshot!(context, context.run().arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.gitmodules', '.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']

    ----- stderr -----
    "#);

    // 2. Test that current directory is in the submodule with a .pre-commit-config
    cmd_snapshot!(context, context.run().current_dir(&submodule_path).arg("--all-files"), @r"
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
    fs_err::remove_file(submodule_path.join(".pre-commit-config.yaml"))?;
    submodule_context.git().add_all().commit("Remove config");

    cmd_snapshot!(context, context.run().current_dir(&submodule_path), @r"
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
fn cookiecutter_template_directories_are_skipped() {
    let context = TestEnv::new_git();

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

    context.write_workspace(["project2", "{{cookiecutter.project_slug}}"], config);

    // Stage only the configs that should participate in discovery.
    context
        .git()
        .add(".pre-commit-config.yaml")
        .add("project2/.pre-commit-config.yaml");

    // The cookiecutter directory would otherwise be discovered as a project.
    cmd_snapshot!(context, context.run().arg("--refresh").arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ project2
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/project2
        ['.pre-commit-config.yaml']
    ✓ <workspace>
      Show CWD...............................................................Passed
      - hook id: show-cwd
      - duration: [TIME]

        [TEMP_DIR]/
        ['.pre-commit-config.yaml', 'project2/.pre-commit-config.yaml']

    ----- stderr -----
    "#);
}

#[test]
fn orphan_projects() {
    let context = TestEnv::new_git();

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

    let context = context
        .with_workspace(["src/backend", "src"], config)
        .with_files([
            ("src/backend/test.py", ""),
            ("src/test.py", ""),
            ("test.py", ""),
        ]);
    context.git().add_all();

    // Without `orphan`: files in subprojects are processed multiple times
    cmd_snapshot!(context, context.run().arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ src/backend
      Show Files.............................................................Passed
      - hook id: show-files
      - duration: [TIME]

        Processing 1 files
          - test.py
    ✓ src
      Show Files.............................................................Passed
      - hook id: show-files
      - duration: [TIME]

        Processing 2 files
          - backend/test.py
          - test.py
    ✓ <workspace>
      Show Files.............................................................Passed
      - hook id: show-files
      - duration: [TIME]

        Processing 3 files
          - src/backend/test.py
          - src/test.py
          - test.py

    ----- stderr -----
    "#);

    // Enable `orphan`
    context.write_file("src/backend/.pre-commit-config.yaml", indoc! {r#"
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
    "#});

    // `files` match nothing, but files are still "consumed"
    context.write_file("src/.pre-commit-config.yaml", indoc! {r#"
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
    "#});

    context.write_file(".pre-commit-config.yaml", indoc! {r#"
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
    "#});

    // In orphan project, files are "consumed" and not processed again in parent projects
    cmd_snapshot!(context, context.run().arg("--all-files"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ src/backend
      Show Files.............................................................Passed
      - hook id: show-files
      - duration: [TIME]

        Processing 1 files
          - test.py
    ✓ src
      Show Files.........................................(no files to check)Skipped
    ✓ <workspace>
      Show Files.............................................................Passed
      - hook id: show-files
      - duration: [TIME]

        Processing 1 files
          - test.py

    ----- stderr -----
    "#);

    // If hooks in orphan projects are not selected, files should be "consumed" as well
    cmd_snapshot!(context, context.run().arg("--all-files").arg("--skip").arg("src/"), @r"
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
}

fn setup_relative_repo_path_project() -> Result<TestEnv> {
    // Create a local hook repository at the root level
    let context = TestEnv::new_git().with_file(
        "hook-repo/.pre-commit-hooks.yaml",
        indoc! {r"
        - id: test-hook
          name: Test Hook
          entry: echo test
          language: system
          always_run: true
        "},
    );
    let hook_repo = context.child("hook-repo");

    let git = context.git_at(&hook_repo);
    git.init().add(".").commit("Initial commit");

    // Get the commit SHA
    let commit_sha = git.rev_parse("HEAD")?;

    // Create a subproject that references the hook repo with a relative path
    // From subproject/, ../hook-repo should resolve to the hook-repo at root
    let context = context
        .with_file(
            "subproject/.pre-commit-config.yaml",
            indoc::formatdoc! {r"
        repos:
          - repo: ../hook-repo
            rev: {commit_sha}
            hooks:
              - id: test-hook
                always_run: true
    "},
        )
        .with_file("subproject/test.txt", "test content")
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: noop
                name: Noop
                entry: echo noop
                language: system
                always_run: true
    "});

    context.git().add_all();

    Ok(context)
}

/// Test that relative repo paths in subproject configs resolve from the config
/// file's directory, not from the process's current working directory.
///
/// Regression test for <https://github.com/j178/prek/issues/1065>
#[test]
fn relative_repo_path_resolution() -> Result<()> {
    let context = setup_relative_repo_path_project()?;

    // Run from the root directory - the relative path ../hook-repo should resolve
    // from subproject/.pre-commit-config.yaml's location, not from CWD
    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ subproject
      Test Hook..............................................................Passed
    ✓ <workspace>
      Noop...................................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn relative_repo_path_resolution_with_explicit_relative_config() -> Result<()> {
    let context = setup_relative_repo_path_project()?;

    cmd_snapshot!(context, context.run()
        .arg("--config")
        .arg("subproject/.pre-commit-config.yaml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Test Hook................................................................Passed

    ----- stderr -----
    "#);

    Ok(())
}
