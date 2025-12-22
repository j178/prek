use std::process::Command;

use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use indoc::indoc;
use prek_consts::CONFIG_FILE;
use prek_consts::env_vars::EnvVars;

use crate::common::TestContext;
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

    cmd_snapshot!(context.filters(), commit, @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1

      always fail

      .pre-commit-config.yaml
    ");
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
        .arg("-c")
        .arg("init.defaultBranch=master")
        .arg("init")
        .arg("--bare")
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

/// Test prek hook runs in the correct worktree.
#[test]
fn run_worktree() -> anyhow::Result<()> {
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
    context.configure_git_author();
    context.disable_auto_crlf();
    context.git_add(".");
    context.git_commit("Initial commit");

    cmd_snapshot!(context.filters(), context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    // Create a new worktree.
    Command::new("git")
        .arg("worktree")
        .arg("add")
        .arg("worktree")
        .arg("HEAD")
        .current_dir(context.work_dir())
        .output()?
        .assert()
        .success();

    // Modify the config in the main worktree
    context.work_dir().child(CONFIG_FILE).write_str("")?;

    let mut commit = Command::new("git");
    commit
        .arg("commit")
        .current_dir(context.work_dir().child("worktree"))
        .arg("-m")
        .arg("Initial commit")
        .arg("--allow-empty");

    cmd_snapshot!(context.filters(), commit, @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1

      always fail
    ");

    Ok(())
}

#[test]
fn workspace_hook_impl_root() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    let config = indoc! {r#"
    repos:
      - repo: local
        hooks:
        - id: test-hook
          name: Test Hook
          language: python
          entry: python -c 'import os; print("cwd:", os.getcwd())'
          verbose: true
    "#};

    context.setup_workspace(&["project2", "project3"], config)?;
    context.git_add(".");

    // Install from root
    cmd_snapshot!(context.filters(), context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    let mut commit = Command::new("git");
    commit
        .current_dir(context.work_dir())
        .arg("commit")
        .arg("-m")
        .arg("Test commit from subdirectory");

    let filters = context
        .filters()
        .into_iter()
        .chain([("[a-f0-9]{7}", "abc1234")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters.clone(), commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] Test commit from subdirectory
     3 files changed, 24 insertions(+)
     create mode 100644 .pre-commit-config.yaml
     create mode 100644 project2/.pre-commit-config.yaml
     create mode 100644 project3/.pre-commit-config.yaml

    ----- stderr -----
    Running hooks for `project2`:
    Test Hook................................................................Passed
    - hook id: test-hook
    - duration: [TIME]

      cwd: [TEMP_DIR]/project2

    Running hooks for `project3`:
    Test Hook................................................................Passed
    - hook id: test-hook
    - duration: [TIME]

      cwd: [TEMP_DIR]/project3

    Running hooks for `.`:
    Test Hook................................................................Passed
    - hook id: test-hook
    - duration: [TIME]

      cwd: [TEMP_DIR]/
    ");

    Ok(())
}

#[test]
fn workspace_hook_impl_subdirectory() -> anyhow::Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    let config = indoc! {r#"
    repos:
      - repo: local
        hooks:
        - id: test-hook
          name: Test Hook
          language: python
          entry: python -c 'import os; print("cwd:", os.getcwd())'
          verbose: true
    "#};

    context.setup_workspace(&["project2", "project3"], config)?;
    context.git_add(".");

    // Install from a subdirectory
    cmd_snapshot!(context.filters(), context.install().current_dir(cwd.join("project2")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `../.git/hooks/pre-commit` for workspace `[TEMP_DIR]/project2`

    hint: this hook installed for `[TEMP_DIR]/project2` only; run `prek install` from `[TEMP_DIR]/` to install for the entire repo.

    ----- stderr -----
    ");

    let mut commit = Command::new("git");
    commit
        .current_dir(cwd)
        .arg("commit")
        .arg("-m")
        .arg("Test commit from subdirectory");

    let filters = context
        .filters()
        .into_iter()
        .chain([("[a-f0-9]{7}", "abc1234")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters.clone(), commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] Test commit from subdirectory
     3 files changed, 24 insertions(+)
     create mode 100644 .pre-commit-config.yaml
     create mode 100644 project2/.pre-commit-config.yaml
     create mode 100644 project3/.pre-commit-config.yaml

    ----- stderr -----
    Running in workspace: `[TEMP_DIR]/project2`
    Test Hook................................................................Passed
    - hook id: test-hook
    - duration: [TIME]

      cwd: [TEMP_DIR]/project2
    ");

    Ok(())
}

/// Install from a subdirectory, and run commit in another worktree.
#[test]
fn workspace_hook_impl_worktree_subdirectory() -> anyhow::Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    let config = indoc! {r#"
    repos:
      - repo: local
        hooks:
        - id: test-hook
          name: Test Hook
          language: python
          entry: python -c 'import os; print("cwd:", os.getcwd())'
          verbose: true
    "#};

    context.setup_workspace(&["project2", "project3"], config)?;
    context.git_add(".");
    context.git_commit("Initial commit");

    // Install from a subdirectory
    cmd_snapshot!(context.filters(), context.install().current_dir(cwd.join("project2")), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `../.git/hooks/pre-commit` for workspace `[TEMP_DIR]/project2`

    hint: this hook installed for `[TEMP_DIR]/project2` only; run `prek install` from `[TEMP_DIR]/` to install for the entire repo.

    ----- stderr -----
    ");

    // Create a new worktree.
    Command::new("git")
        .arg("worktree")
        .arg("add")
        .arg("worktree")
        .arg("HEAD")
        .current_dir(cwd)
        .output()?
        .assert()
        .success();

    // Modify the config in the main worktree
    context
        .work_dir()
        .child("project2")
        .child(CONFIG_FILE)
        .write_str("")?;

    let mut commit = Command::new("git");
    commit
        .current_dir(cwd.child("worktree"))
        .env(EnvVars::PREK_HOME, &**context.home_dir())
        .arg("commit")
        .arg("-m")
        .arg("Test commit from subdirectory")
        .arg("--allow-empty");

    let filters = context
        .filters()
        .into_iter()
        .chain([("[a-f0-9]{7}", "abc1234")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters.clone(), commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [detached HEAD abc1234] Test commit from subdirectory

    ----- stderr -----
    Running in workspace: `[TEMP_DIR]/worktree/project2`
    Unstaged changes detected, stashing unstaged changes to `[HOME]/patches/abc1234568636-80689.patch`
    Test Hook............................................(no files to check)Skipped
    Restored working tree changes from `[HOME]/patches/abc1234568636-80689.patch`
    ");

    let log = fs_err::read_to_string(context.home_dir().join("prek.log"))?;
    insta::assert_snapshot!(log, @r#"
    2025-12-22T10:32:48.299260Z DEBUG prek: 0.2.23
    2025-12-22T10:32:48.314527Z TRACE get_root: close time.busy=15.2ms time.idle=9.46µs
    2025-12-22T10:32:48.314591Z DEBUG Git root: /Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree
    2025-12-22T10:32:48.314616Z DEBUG Changing current directory to: `project2`
    2025-12-22T10:32:48.314638Z DEBUG Args: ["/Users/Jo/code/rust/prek/target/debug/prek", "hook-impl", "--hook-type=pre-commit", "--cd=project2", "--script-version=4", "--hook-dir", "/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/.git/hooks", "--"]
    2025-12-22T10:32:48.314988Z DEBUG Found workspace root at `/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2`
    2025-12-22T10:32:48.315015Z DEBUG Found project root at ``
    2025-12-22T10:32:48.315037Z DEBUG Loading project configuration path=.pre-commit-config.yaml
    2025-12-22T10:32:48.315627Z TRACE read_config{path="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2/.pre-commit-config.yaml"}: close time.busy=547µs time.idle=5.33µs
    2025-12-22T10:32:48.315784Z TRACE Executing `/opt/homebrew/opt/git/libexec/git-core/git ls-files --unmerged`
    2025-12-22T10:32:48.332067Z DEBUG Found workspace root at `/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2`
    2025-12-22T10:32:48.332131Z TRACE Include selectors: ``
    2025-12-22T10:32:48.332152Z TRACE Skip selectors: ``
    2025-12-22T10:32:48.332248Z DEBUG discover{root="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2" config=None refresh=false}: Performing fresh workspace discovery
    2025-12-22T10:32:48.332335Z TRACE discover{root="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2" config=None refresh=false}:list_submodules{git_root="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree"}: close time.busy=8.00µs time.idle=5.42µs
    2025-12-22T10:32:48.337096Z DEBUG Loading project configuration path=.pre-commit-config.yaml
    2025-12-22T10:32:48.337556Z TRACE read_config{path="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2/.pre-commit-config.yaml"}: close time.busy=405µs time.idle=6.21µs
    2025-12-22T10:32:48.341044Z TRACE discover{root="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2" config=None refresh=false}: close time.busy=8.84ms time.idle=8.21µs
    2025-12-22T10:32:48.341118Z TRACE Executing `/opt/homebrew/opt/git/libexec/git-core/git diff --exit-code --name-only -z /Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2/.pre-commit-config.yaml [...]`
    2025-12-22T10:32:48.359838Z TRACE Checking lock resource="store" path=/Users/Jo/.local/share/prek/tests/.tmpy22cNf/home/.lock
    2025-12-22T10:32:48.359886Z DEBUG Acquired lock resource="store"
    2025-12-22T10:32:48.360326Z DEBUG Hooks going to run: ["test-hook"]
    2025-12-22T10:32:48.361058Z DEBUG Found uv in PATH: /Users/Jo/.local/bin/uv
    2025-12-22T10:32:48.379968Z TRACE Using system uv version 0.9.11 at /Users/Jo/.local/bin/uv
    2025-12-22T10:32:48.380281Z DEBUG Installing environment hook=test-hook target=/Users/Jo/.local/share/prek/tests/.tmpy22cNf/home/hooks/python-RCFClN394zEIMCQ8ojzA
    2025-12-22T10:32:48.380318Z TRACE Executing `/Users/Jo/.local/bin/uv venv /Users/Jo/.local/share/prek/tests/.tmpy22cNf/home/hooks/python-RCFClN394zEIMCQ8ojzA --python-preference managed --no-project [...]`
    2025-12-22T10:32:48.579815Z DEBUG Venv created successfully with no downloads: `/Users/Jo/.local/share/prek/tests/.tmpy22cNf/home/hooks/python-RCFClN394zEIMCQ8ojzA`
    2025-12-22T10:32:48.579897Z DEBUG No dependencies to install
    2025-12-22T10:32:48.579923Z TRACE Executing `/Users/Jo/.local/share/prek/tests/.tmpy22cNf/home/hooks/python-RCFClN394zEIMCQ8ojzA/bin/python -I -c import sys, json
    info = {
        "version": ".".join(map(str, sys.version_info[:3])),
        "base_exec_prefix": sys.base_exec_prefix,
    }
    print(json.dumps(info))
     [...]`
    2025-12-22T10:32:48.603471Z DEBUG Installed hook `test-hook` in `/Users/Jo/.local/share/prek/tests/.tmpy22cNf/home/hooks/python-RCFClN394zEIMCQ8ojzA`
    2025-12-22T10:32:48.603564Z TRACE Released lock path=/Users/Jo/.local/share/prek/tests/.tmpy22cNf/home/.lock
    2025-12-22T10:32:48.603630Z TRACE Executing `/opt/homebrew/opt/git/libexec/git-core/git diff --diff-filter=A --name-only -z -- /Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2`
    2025-12-22T10:32:48.613621Z TRACE Executing `/opt/homebrew/opt/git/libexec/git-core/git write-tree`
    2025-12-22T10:32:48.624428Z TRACE Executing `/opt/homebrew/opt/git/libexec/git-core/git diff-index --binary --exit-code 44e303738875ee49c589134dfa566c18df7b4b3c -- /Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2 [...]`
    2025-12-22T10:32:48.636653Z DEBUG Unstaged changes detected
    2025-12-22T10:32:48.637081Z DEBUG Cleaning working tree
    2025-12-22T10:32:48.649884Z TRACE collect_files: Executing `/opt/homebrew/opt/git/libexec/git-core/git rev-parse --git-dir`
    2025-12-22T10:32:48.658905Z TRACE collect_files: Executing `cd /Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2 && /opt/homebrew/opt/git/libexec/git-core/git diff --cached --name-only --diff-filter=ACMRTUXB -z`
    2025-12-22T10:32:48.669322Z DEBUG collect_files: Staged files: 0
    2025-12-22T10:32:48.669376Z TRACE collect_files: close time.busy=777µs time.idle=18.7ms
    2025-12-22T10:32:48.669519Z TRACE for_project{project=.}: close time.busy=2.92µs time.idle=4.17µs
    2025-12-22T10:32:48.669560Z TRACE Files for project `.` after filtered: 0
    2025-12-22T10:32:48.669596Z TRACE get_diff{path="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2"}: Executing `/opt/homebrew/opt/git/libexec/git-core/git diff -- /Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2`
    2025-12-22T10:32:48.680014Z TRACE get_diff{path="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2"}: close time.busy=273µs time.idle=10.1ms
    2025-12-22T10:32:48.680354Z TRACE for_hook{hook="test-hook"}: close time.busy=227µs time.idle=3.25µs
    2025-12-22T10:32:48.680387Z TRACE Files for hook `test-hook` after filtered: 0
    2025-12-22T10:32:48.680423Z TRACE get_diff{path="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2"}: Executing `/opt/homebrew/opt/git/libexec/git-core/git diff -- /Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2`
    2025-12-22T10:32:48.691763Z TRACE get_diff{path="/Users/Jo/.local/share/prek/tests/.tmpy22cNf/temp/worktree/project2"}: close time.busy=292µs time.idle=11.1ms
    "#);

    Ok(())
}

#[test]
fn workspace_hook_impl_no_project_found() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    // Create a directory without .pre-commit-config.yaml
    let empty_dir = context.work_dir().child("empty");
    empty_dir.create_dir_all()?;
    empty_dir.child("file.txt").write_str("Some content")?;
    context.git_add(".");

    // Install hook that allows missing config
    cmd_snapshot!(context.filters(), context.install(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-commit`

    ----- stderr -----
    ");

    // Try to run hook-impl from directory without config
    let mut commit = Command::new("git");
    commit
        .current_dir(&empty_dir)
        .arg("commit")
        .arg("-m")
        .arg("Test commit");

    cmd_snapshot!(context.filters(), commit, @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: No `.pre-commit-config.yaml` found in the current directory or parent directories.

    hint: If you just added one, rerun your command with the `--refresh` flag to rescan the workspace.
    - To temporarily silence this, run `PREK_ALLOW_NO_CONFIG=1 git ...`
    - To permanently silence this, install hooks with the `--allow-missing-config` flag
    - To uninstall hooks, run `prek uninstall`
    ");

    // Commit with `PREK_ALLOW_NO_CONFIG=1`
    let mut commit = Command::new("git");
    commit
        .current_dir(&empty_dir)
        .env(EnvVars::PREK_ALLOW_NO_CONFIG, "1")
        .arg("commit")
        .arg("-m")
        .arg("Test commit");

    let filters = context
        .filters()
        .into_iter()
        .chain([("[a-f0-9]{7}", "1d5e501")])
        .collect::<Vec<_>>();

    // The hook should simply succeed because there is no config
    cmd_snapshot!(filters.clone(), commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) 1d5e501] Test commit
     1 file changed, 1 insertion(+)
     create mode 100644 empty/file.txt

    ----- stderr -----
    ");

    // Create the root `.pre-commit-config.yaml`
    context
        .work_dir()
        .child(CONFIG_FILE)
        .write_str(indoc::indoc! {r"
        repos:
        - repo: local
          hooks:
           - id: fail
             name: fail
             entry: fail
             language: fail
    "})?;
    context.git_add(".");

    // Commit with `PREK_ALLOW_NO_CONFIG=1` again, the hooks should run (and fail)
    let mut commit = Command::new("git");
    commit
        .current_dir(&empty_dir)
        .env(EnvVars::PREK_ALLOW_NO_CONFIG, "1")
        .arg("commit")
        .arg("-m")
        .arg("Test commit");

    cmd_snapshot!(filters.clone(), commit, @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1

      fail

      .pre-commit-config.yaml
    ");

    Ok(())
}

#[test]
fn workspace_hook_impl_with_selectors() -> anyhow::Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();
    context.configure_git_author();
    context.disable_auto_crlf();

    let config = indoc! {r#"
    repos:
      - repo: local
        hooks:
        - id: test-hook
          name: Test Hook
          language: python
          entry: python -c 'import os; print("cwd:", os.getcwd())'
          verbose: true
    "#};

    context.setup_workspace(&["project2", "project3"], config)?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.install().arg("project2/"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-commit`

    ----- stderr -----
    ");

    let mut commit = Command::new("git");
    commit
        .current_dir(cwd)
        .arg("commit")
        .arg("-m")
        .arg("Test commit from subdirectory");

    let filters = context
        .filters()
        .into_iter()
        .chain([("[a-f0-9]{7}", "abc1234")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters.clone(), commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] Test commit from subdirectory
     3 files changed, 24 insertions(+)
     create mode 100644 .pre-commit-config.yaml
     create mode 100644 project2/.pre-commit-config.yaml
     create mode 100644 project3/.pre-commit-config.yaml

    ----- stderr -----
    Running hooks for `project2`:
    Test Hook................................................................Passed
    - hook id: test-hook
    - duration: [TIME]

      cwd: [TEMP_DIR]/project2
    ");

    Ok(())
}
