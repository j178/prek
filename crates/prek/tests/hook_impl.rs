#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::path::Path;

use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use indoc::indoc;
use prek_consts::PRE_COMMIT_CONFIG_YAML;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot, git_cmd};

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

    let mut commit = git_cmd(context.work_dir());
    commit
        .arg("commit")
        .env(EnvVars::PREK_HOME, &**context.home_dir())
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

    let mut commit = git_cmd(context.work_dir());
    commit
        .arg("commit")
        .env(EnvVars::PREK_HOME, &**context.home_dir())
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

    let mut init_remote = git_cmd(&remote_repo_path);
    init_remote
        .arg("-c")
        .arg("init.defaultBranch=master")
        .arg("init")
        .arg("--bare");
    cmd_snapshot!(context.filters(), init_remote, @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Initialized empty Git repository in [HOME]/remote.git/

    ----- stderr -----
    "#);

    // Add remote to local repo
    let mut add_remote = git_cmd(context.work_dir());
    add_remote
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(&remote_repo_path);
    cmd_snapshot!(context.filters(), add_remote, @r#"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    "#);

    // First push - should trigger the hook
    let mut push_cmd = git_cmd(context.work_dir());
    push_cmd
        .arg("push")
        .arg("origin")
        .arg("master")
        .env(EnvVars::PREK_HOME, &**context.home_dir());

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
    let mut push_cmd2 = git_cmd(context.work_dir());
    push_cmd2
        .arg("push")
        .arg("origin")
        .arg("master")
        .env(EnvVars::PREK_HOME, &**context.home_dir());

    cmd_snapshot!(context.filters(), push_cmd2, @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Everything up-to-date
    ");

    Ok(())
}

#[test]
fn hook_impl_runs_legacy_hook() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YAML)
        .write_str(indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: manual-only
                name: manual-only
                language: system
                entry: echo manual-only
                stages: [ manual ]
  "})?;

    context.work_dir().child("file.txt").write_str("x")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.install(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-commit`

    ----- stderr -----
  ");

    let legacy_hook = context.work_dir().child(".git/hooks/pre-commit.legacy");
    legacy_hook.write_str(indoc::indoc! {r#"
        #!/bin/sh
        python3 -c 'print("legacy pre-commit ran")'
        exit 1
    "#})?;
    #[cfg(unix)]
    set_executable(legacy_hook.path())?;

    let mut commit = git_cmd(context.work_dir());
    commit
        .env(EnvVars::PREK_HOME, &**context.home_dir())
        .arg("commit")
        .arg("-m")
        .arg("Test commit");

    cmd_snapshot!(context.filters(), commit, @"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    legacy pre-commit ran
    ");

    Ok(())
}

#[test]
fn hook_impl_pre_push_runs_legacy_and_prek() -> anyhow::Result<()> {
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
    context.git_commit("Initial commit");

    cmd_snapshot!(context.filters(), context.install().arg("--hook-type").arg("pre-push"), @r#"
  success: true
  exit_code: 0
  ----- stdout -----
  prek installed at `.git/hooks/pre-push`

  ----- stderr -----
  "#);

    let legacy_hook = context.work_dir().child(".git/hooks/pre-push.legacy");
    legacy_hook.write_str(indoc::indoc! {r#"
        #!/bin/sh
        python3 -c 'print("legacy pre-push ran")'
        exit 1
    "#})?;
    #[cfg(unix)]
    set_executable(legacy_hook.path())?;

    let remote_repo_path = context.home_dir().join("remote.git");
    std::fs::create_dir_all(&remote_repo_path)?;

    let mut init_remote = git_cmd(&remote_repo_path);
    init_remote
        .arg("-c")
        .arg("init.defaultBranch=master")
        .arg("init")
        .arg("--bare");
    init_remote.output()?.assert().success();

    let mut add_remote = git_cmd(context.work_dir());
    add_remote
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(&remote_repo_path);
    add_remote.output()?.assert().success();

    context.work_dir().child("file.txt").write_str("x")?;
    context.git_add(".");
    context.git_commit("Second commit");

    let mut push_cmd = git_cmd(context.work_dir());
    push_cmd
        .arg("push")
        .arg("origin")
        .arg("master")
        .env(EnvVars::PREK_HOME, &**context.home_dir());

    cmd_snapshot!(context.filters(), push_cmd, @"
    success: false
    exit_code: 1
    ----- stdout -----
    legacy pre-push ran
    success..................................................................Passed

    ----- stderr -----
    error: failed to push some refs to '[HOME]/remote.git'
    ");

    Ok(())
}

#[cfg(unix)]
fn set_executable(path: &Path) -> anyhow::Result<()> {
    let mut perms = fs_err::metadata(path)?.permissions();
    perms.set_mode(0o755);
    fs_err::set_permissions(path, perms)?;
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
    git_cmd(context.work_dir())
        .arg("worktree")
        .arg("add")
        .arg("worktree")
        .arg("HEAD")
        .output()?
        .assert()
        .success();

    // Modify the config in the main worktree
    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YAML)
        .write_str("")?;

    let mut commit = git_cmd(context.work_dir().child("worktree"));
    commit
        .arg("commit")
        .env(EnvVars::PREK_HOME, &**context.home_dir())
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

/// Test prek hooks runs with `GIT_DIR` respected.
#[test]
fn git_dir_respected() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc! { r#"
        repos:
        - repo: local
          hooks:
           - id: print-git-dir
             name: Print Git Dir
             language: python
             entry: python -c 'import os, sys; print("GIT_DIR:", os.environ.get("GIT_DIR")); print("GIT_WORK_TREE:", os.environ.get("GIT_WORK_TREE")); sys.exit(1)'
             pass_filenames: false
    "#});
    context.git_add(".");
    let cwd = context.work_dir();

    cmd_snapshot!(context.filters(), context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    let mut commit = git_cmd(context.home_dir());
    commit
        .arg("--git-dir")
        .arg(cwd.join(".git"))
        .arg("--work-tree")
        .arg(&**cwd)
        .env(EnvVars::PREK_HOME, &**context.home_dir())
        .arg("commit")
        .arg("-m")
        .arg("Test commit with GIT_DIR set");

    cmd_snapshot!(context.filters(), commit, @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    Print Git Dir............................................................Failed
    - hook id: print-git-dir
    - exit code: 1

      GIT_DIR: [TEMP_DIR]/.git
      GIT_WORK_TREE: .
    ");
}

#[test]
fn workspace_hook_impl_root() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

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

    let mut commit = git_cmd(context.work_dir());
    commit
        .env(EnvVars::PREK_HOME, &**context.home_dir())
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

    let mut commit = git_cmd(cwd);
    commit
        .env(EnvVars::PREK_HOME, &**context.home_dir())
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
    git_cmd(cwd)
        .arg("worktree")
        .arg("add")
        .arg("worktree")
        .arg("HEAD")
        .output()?
        .assert()
        .success();

    // Modify the config in the main worktree
    context
        .work_dir()
        .child("project2")
        .child(PRE_COMMIT_CONFIG_YAML)
        .write_str("")?;

    let mut commit = git_cmd(cwd.child("worktree"));
    commit
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
    Test Hook............................................(no files to check)Skipped
    ");

    Ok(())
}

#[test]
fn workspace_hook_impl_no_project_found() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

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
    let mut commit = git_cmd(&empty_dir);
    commit
        .env(EnvVars::PREK_HOME, &**context.home_dir())
        .arg("commit")
        .arg("-m")
        .arg("Test commit");

    cmd_snapshot!(context.filters(), commit, @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    error: No `prek.toml` or `.pre-commit-config.yaml` found in the current directory or parent directories.

    hint: If you just added one, rerun your command with the `--refresh` flag to rescan the workspace.
    - To temporarily silence this, run `PREK_ALLOW_NO_CONFIG=1 git ...`
    - To permanently silence this, install hooks with the `--allow-missing-config` flag
    - To uninstall hooks, run `prek uninstall`
    ");

    // Commit with `PREK_ALLOW_NO_CONFIG=1`
    let mut commit = git_cmd(&empty_dir);
    commit
        .env(EnvVars::PREK_HOME, &**context.home_dir())
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
        .child(PRE_COMMIT_CONFIG_YAML)
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
    let mut commit = git_cmd(&empty_dir);
    commit
        .env(EnvVars::PREK_HOME, &**context.home_dir())
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
fn hook_impl_does_not_fail_when_no_hooks_match_stage() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    // Only a manual-stage hook; a pre-commit hook run should find nothing for the stage.
    context
        .work_dir()
        .child(PRE_COMMIT_CONFIG_YAML)
        .write_str(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: manual-only
                name: manual-only
                language: system
                entry: echo manual-only
                stages: [ manual ]
    "})?;

    context.work_dir().child("file.txt").write_str("x")?;
    context.git_add(".");

    // Install the git hook (which invokes `prek hook-impl`).
    cmd_snapshot!(context.filters(), context.install(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    prek installed at `.git/hooks/pre-commit`

    ----- stderr -----
    ");

    // Commit should succeed; the hook should not error just because no hooks match pre-commit.
    let mut commit = git_cmd(context.work_dir());
    commit
        .env(EnvVars::PREK_HOME, &**context.home_dir())
        .arg("commit")
        .arg("-m")
        .arg("Test commit");

    let filters = context
        .filters()
        .into_iter()
        .chain([("[a-f0-9]{7}", "abc1234")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] Test commit
     2 files changed, 9 insertions(+)
     create mode 100644 .pre-commit-config.yaml
     create mode 100644 file.txt

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn workspace_hook_impl_with_selectors() -> anyhow::Result<()> {
    let context = TestContext::new();
    let cwd = context.work_dir();
    context.init_project();

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

    let mut commit = git_cmd(cwd);
    commit
        .env(EnvVars::PREK_HOME, &**context.home_dir())
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
