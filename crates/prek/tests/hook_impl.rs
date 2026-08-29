use assert_cmd::assert::OutputAssertExt;
use indoc::indoc;
use prek_consts::PRE_COMMIT_CONFIG_YAML;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot};

mod common;

#[test]
fn hook_impl() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
        repos:
        - repo: local
          hooks:
           - id: fail
             name: fail
             language: fail
             entry: always fail
             always_run: true
    "});

    context.git().add_all();

    let mut commit = context.git().command();
    commit.arg("commit").arg("-m").arg("Initial commit");

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    cmd_snapshot!(context, commit, @r"
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
fn hook_impl_allows_missing_hook_dir() {
    let context = TestEnv::new_git()
        .with_config(indoc::indoc! {r#"
        repos:
        - repo: local
          hooks:
           - id: success
             name: success
             language: system
             entry: echo "hook ran successfully"
             always_run: true
    "#})
        .with_executable_file(
            ".git/hooks/pre-commit.legacy",
            indoc::indoc! {r#"
        #!/bin/sh
        python3 -c 'print("legacy pre-commit ran")'
        exit 1
        "#},
        );

    // Git 2.54+ config-based hooks can invoke `hook-impl` without
    // `--hook-dir`; without a hook script directory, legacy hooks are skipped.
    let mut hook_impl = context.command();
    hook_impl
        .arg("hook-impl")
        .arg("--hook-type")
        .arg("pre-commit")
        .arg("--script-version")
        .arg("4");

    cmd_snapshot!(context, hook_impl, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    success..................................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn hook_impl_pre_push() -> anyhow::Result<()> {
    let context = TestEnv::new_git()
        .with_filter(r"\b[0-9a-f]{7}\b", "[SHA1]")
        .with_config(indoc::indoc! {r#"
        repos:
        - repo: local
          hooks:
           - id: success
             name: success
             language: system
             entry: echo "hook ran successfully"
             always_run: true
    "#});
    context.git().add_all();

    let mut commit = context.git().command();
    commit.arg("commit").arg("-m").arg("Initial commit");

    cmd_snapshot!(context, context.install().arg("--hook-type").arg("pre-push"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-push`

    ----- stderr -----
    "#);

    cmd_snapshot!(context, commit, @r"
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
    fs_err::create_dir_all(&remote_repo_path)?;

    let mut init_remote = context.git_at(&remote_repo_path).command();
    init_remote
        .arg("-c")
        .arg("init.defaultBranch=master")
        .arg("init")
        .arg("--bare");
    cmd_snapshot!(context, init_remote, @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Initialized empty Git repository in [HOME]/remote.git/

    ----- stderr -----
    "#);

    // Add remote to local repo
    let mut add_remote = context.git().command();
    add_remote
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(&remote_repo_path);
    cmd_snapshot!(context, add_remote, @r#"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    "#);

    // First push - should trigger the hook
    let mut push_cmd = context.git().command();
    push_cmd.arg("push").arg("origin").arg("master");

    cmd_snapshot!(context, push_cmd, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    success..................................................................Passed

    ----- stderr -----
    To [HOME]/remote.git
     * [new branch]      master -> master
    ");

    // Second push - should not trigger the hook (nothing new to push)
    let mut push_cmd2 = context.git().command();
    push_cmd2.arg("push").arg("origin").arg("master");

    cmd_snapshot!(context, push_cmd2, @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Everything up-to-date
    ");

    Ok(())
}

#[test]
fn hook_impl_pre_push_force_push_after_rebase() -> anyhow::Result<()> {
    let context = TestEnv::new_git();

    // Regression test for https://github.com/j178/prek/issues/2088.
    // The hook fails after printing its filenames so the assertion can inspect
    // the exact file set selected by the pre-push range calculation.
    let context = context
        .with_config(indoc::indoc! {r"
        repos:
        - repo: local
          hooks:
           - id: filenames
             name: filenames
             language: system
             entry: python3 print_filenames.py
             stages: [ pre-push ]
    "})
        .with_file(
            "print_filenames.py",
            indoc! { r"
            import sys

            for filename in sys.argv[1:]:
                print(filename)

            raise SystemExit(1)
        "},
        );

    context.git().add_all().commit("Initial commit");

    let remote_repo_path = context.home_dir().join("remote.git");
    fs_err::create_dir_all(&remote_repo_path)?;

    context
        .git_at(&remote_repo_path)
        .run(["-c", "init.defaultBranch=master", "init", "--bare"]);

    let mut add_remote = context.git().command();
    add_remote
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(&remote_repo_path);
    add_remote.output()?.assert().success();

    context.git().run(["push", "origin", "master"]);

    // Create and push a feature branch so the remote has an old feature tip.
    // This old tip is what Git passes to pre-push as remote_sha during the
    // later force-push.
    context.git().run(["checkout", "-b", "feature"]);

    context.write_file("feature.txt", "feature");
    context.git().add_all().commit("Add feature file");

    context.git().run(["push", "origin", "feature"]);

    // Move master forward with an unrelated file. After the feature branch is
    // rebased onto this commit, main.txt must not appear in the pre-push file
    // list because it is default-branch churn, not a feature-branch change.
    context.git().checkout("master");
    context.write_file("main.txt", "main");
    context.git().add_all().commit("Update master");

    context
        .git()
        .run(["push", "origin", "master"])
        .run(["fetch", "origin"]);

    context.git().checkout("feature");

    // Rebase rewrites the feature commit, so the old remote feature tip still
    // exists locally but is no longer an ancestor of the new local feature tip.
    // That is the #2088 shape that used to make old_remote...new_local include
    // unrelated master changes.
    context.git().run(["rebase", "origin/master"]);

    context
        .install()
        .arg("--hook-type")
        .arg("pre-push")
        .output()?
        .assert()
        .success();

    let mut push_cmd = context.git().command();
    push_cmd
        .arg("push")
        .arg("--force")
        .arg("origin")
        .arg("feature");

    // The correct range is origin/master...feature after the rebase, so only
    // feature.txt should be passed to hooks. If the old remote feature tip were
    // used as from_ref, this would also include main.txt.
    cmd_snapshot!(context, push_cmd, @r"
    success: false
    exit_code: 1
    ----- stdout -----
    filenames................................................................Failed
    - hook id: filenames
    - exit code: 1

      feature.txt

    ----- stderr -----
    error: failed to push some refs to '[HOME]/remote.git'
    ");

    Ok(())
}

#[test]
fn hook_impl_runs_legacy_hook() {
    let context = TestEnv::new_git()
        .with_file(
            PRE_COMMIT_CONFIG_YAML,
            indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: manual-only
                name: manual-only
                language: system
                entry: echo manual-only
                stages: [ manual ]
  "},
        )
        .with_file("file.txt", "x");

    context.git().add_all();

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    context.write_executable_file(
        ".git/hooks/pre-commit.legacy",
        indoc::indoc! {r#"
        #!/bin/sh
        python3 -c 'print("legacy pre-commit ran")'
        exit 1
    "#},
    );
    let mut commit = context.git().command();
    commit.arg("commit").arg("-m").arg("Test commit");

    cmd_snapshot!(context, commit, @"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    legacy pre-commit ran
    ");
}

#[test]
fn hook_impl_pre_push_runs_legacy_and_prek() -> anyhow::Result<()> {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
    repos:
    - repo: local
      hooks:
          - id: success
            name: success
            language: system
            entry: echo "hook ran successfully"
            always_run: true
  "#});
    context.git().add_all().commit("Initial commit");

    cmd_snapshot!(context, context.install().arg("--hook-type").arg("pre-push"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-push`

    ----- stderr -----
    "#);

    context.write_executable_file(
        ".git/hooks/pre-push.legacy",
        indoc::indoc! {r#"
        #!/bin/sh
        python3 -c 'print("legacy pre-push ran")'
        exit 1
    "#},
    );
    let remote_repo_path = context.home_dir().join("remote.git");
    fs_err::create_dir_all(&remote_repo_path)?;

    let mut init_remote = context.git_at(&remote_repo_path).command();
    init_remote
        .arg("-c")
        .arg("init.defaultBranch=master")
        .arg("init")
        .arg("--bare");
    init_remote.output()?.assert().success();

    let mut add_remote = context.git().command();
    add_remote
        .arg("remote")
        .arg("add")
        .arg("origin")
        .arg(&remote_repo_path);
    add_remote.output()?.assert().success();

    context.write_file("file.txt", "x");
    context.git().add_all().commit("Second commit");

    let mut push_cmd = context.git().command();
    push_cmd.arg("push").arg("origin").arg("master");

    cmd_snapshot!(context, push_cmd, @"
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

/// Test prek hook runs in the correct worktree.
#[test]
fn run_worktree() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
        repos:
        - repo: local
          hooks:
           - id: fail
             name: fail
             language: fail
             entry: always fail
             always_run: true
    "});
    context.git().add_all().commit("Initial commit");

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    // Create a new worktree.
    context.git().run(["worktree", "add", "worktree", "HEAD"]);

    // Modify the config in the main worktree
    context.write_file(PRE_COMMIT_CONFIG_YAML, "");

    let mut commit = context.git_at(context.child("worktree")).command();
    commit
        .arg("commit")
        .arg("-m")
        .arg("Initial commit")
        .arg("--allow-empty");

    cmd_snapshot!(context, commit, @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    fail.....................................................................Failed
    - hook id: fail
    - exit code: 1

      always fail
    ");
}

/// Test prek hooks runs with `GIT_DIR` respected.
#[test]
fn git_dir_respected() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
        repos:
        - repo: local
          hooks:
           - id: print-git-dir
             name: Print Git Dir
             language: python
             entry: python -c 'import os, sys; print("GIT_DIR:", os.environ.get("GIT_DIR")); print("GIT_WORK_TREE:", os.environ.get("GIT_WORK_TREE")); sys.exit(1)'
             pass_filenames: false
    "#});
    context.git().add_all();
    let cwd = context.work_dir();

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    let mut commit = context.git_at(context.home_dir()).command();
    commit
        .arg("--git-dir")
        .arg(cwd.join(".git"))
        .arg("--work-tree")
        .arg(&**cwd)
        .arg("commit")
        .arg("-m")
        .arg("Test commit with GIT_DIR set");

    cmd_snapshot!(context, commit, @r"
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
fn git_dir_synthesized_git_work_tree_not_leaked_to_hook() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r#"
        repos:
        - repo: local
          hooks:
           - id: print-git-work-tree
             name: Print Git Work Tree
             language: system
             entry: python3 -c 'import os, sys; print("GIT_DIR:", os.environ.get("GIT_DIR")); print("GIT_WORK_TREE:", os.environ.get("GIT_WORK_TREE")); sys.exit(1)'
             pass_filenames: false
             always_run: true
    "#});
    context.git().add_all();

    let mut run = context.run();
    run.env(EnvVars::GIT_DIR, context.work_dir().join(".git"));

    cmd_snapshot!(context, run, @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Print Git Work Tree......................................................Failed
    - hook id: print-git-work-tree
    - exit code: 1

      GIT_DIR: [TEMP_DIR]/.git
      GIT_WORK_TREE: None

    ----- stderr -----
    ");
}

/// Committing from a linked worktree makes Git export an absolute `GIT_DIR` without a
/// `GIT_WORK_TREE`, so Git falls back to treating the hook's working directory as the
/// work tree root. A workspace hook runs in its own project directory, so a Git command
/// it runs must still resolve the repository root, not the project directory.
#[test]
fn workspace_hook_in_linked_worktree_keeps_git_index() {
    let context = TestEnv::new_git()
        .with_filter("[a-f0-9]{7}", "abc1234")
        .with_config(indoc::indoc! {r"
        repos:
        - repo: local
          hooks:
           - id: root-noop
             name: root-noop
             language: system
             entry: true
             always_run: true
             pass_filenames: false
    "})
        .with_file(
            "sub/.pre-commit-config.yaml",
            indoc! { r"
        repos:
        - repo: local
          hooks:
           - id: sub-git-add
             name: sub-git-add
             language: system
             entry: git add -u
             always_run: true
             pass_filenames: false
    "},
        )
        .with_file("README.md", "root\n")
        .with_file("tracked.txt", "b\n")
        .with_file("sub/README.md", "sub\n")
        .with_file("deep/nested/a.txt", "a\n");

    context.git().add_all().commit("Initial commit");

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    context.git().run(["worktree", "add", "worktree", "HEAD"]);

    let worktree = context.child("worktree");
    context.write_file("worktree/tracked.txt", "b2\n");
    context.git_at(&worktree).add("tracked.txt");

    let mut commit = context.git_at(&worktree).command();
    commit
        .arg("commit")
        .arg("-m")
        .arg("Commit from the linked worktree");

    cmd_snapshot!(context, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [detached HEAD abc1234] Commit from the linked worktree
     1 file changed, 1 insertion(+), 1 deletion(-)

    ----- stderr -----
    ✓ sub
      sub-git-add............................................................Passed
    ✓ <workspace>
      root-noop..............................................................Passed
    ");

    // The hook's `git add -u` must not rewrite the index as if `sub` were the repository.
    let mut ls_files = context.git_at(&worktree).command();
    ls_files.arg("ls-files");
    cmd_snapshot!(context, ls_files, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    .pre-commit-config.yaml
    README.md
    deep/nested/a.txt
    sub/.pre-commit-config.yaml
    sub/README.md
    tracked.txt

    ----- stderr -----
    ");

    let mut status = context.git_at(&worktree).command();
    status.args(["status", "--porcelain"]);
    cmd_snapshot!(context, status, @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");
}

#[test]
fn workspace_hook_impl_root() {
    let context = TestEnv::new_git().with_filter("[a-f0-9]{7}", "abc1234");

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

    context.write_workspace(["project2", "project3"], config);
    context.git().add_all();

    // Install from root
    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    let mut commit = context.git().command();
    commit
        .arg("commit")
        .arg("-m")
        .arg("Test commit from subdirectory");

    cmd_snapshot!(context, commit, @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] Test commit from subdirectory
     3 files changed, 24 insertions(+)
     create mode 100644 .pre-commit-config.yaml
     create mode 100644 project2/.pre-commit-config.yaml
     create mode 100644 project3/.pre-commit-config.yaml

    ----- stderr -----
    ✓ project2
      Test Hook..............................................................Passed
      - hook id: test-hook
      - duration: [TIME]

        cwd: [TEMP_DIR]/project2
    ✓ project3
      Test Hook..............................................................Passed
      - hook id: test-hook
      - duration: [TIME]

        cwd: [TEMP_DIR]/project3
    ✓ <workspace>
      Test Hook..............................................................Passed
      - hook id: test-hook
      - duration: [TIME]

        cwd: [TEMP_DIR]/
    "#);
}

#[test]
fn workspace_commit_msg_hook_receives_message_file_for_each_project() {
    let context = TestEnv::new_git().with_filter("[a-f0-9]{7}", "abc1234");

    let config = indoc! {r#"
    default_install_hook_types:
      - commit-msg
    repos:
      - repo: local
        hooks:
        - id: commit-msg-args
          name: Commit Msg Args
          language: python
          entry: python -c 'import os, pathlib, sys; print("cwd:", os.getcwd()); print("args:", sys.argv[1:]); assert len(sys.argv) == 2; assert pathlib.Path(sys.argv[1]).is_file()'
          stages: [commit-msg]
          always_run: true
          verbose: true
    "#};

    context.write_workspace(["template"], config);
    context.git().add_all();

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/commit-msg`

    ----- stderr -----
    "#);

    let mut commit = context.git().command();
    commit.arg("commit").arg("-m").arg("feat: initial");

    cmd_snapshot!(context, commit, @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] feat: initial
     2 files changed, 24 insertions(+)
     create mode 100644 .pre-commit-config.yaml
     create mode 100644 template/.pre-commit-config.yaml

    ----- stderr -----
    ✓ template
      Commit Msg Args........................................................Passed
      - hook id: commit-msg-args
      - duration: [TIME]

        cwd: [TEMP_DIR]/template
        args: ['../.git/COMMIT_EDITMSG']
    ✓ <workspace>
      Commit Msg Args........................................................Passed
      - hook id: commit-msg-args
      - duration: [TIME]

        cwd: [TEMP_DIR]/
        args: ['.git/COMMIT_EDITMSG']
    "#);
}

#[test]
fn commit_msg_builtin_hook_respects_message_file_filters() {
    let context = TestEnv::new_git()
        .with_filter("[a-f0-9]{7}", "abc1234")
        .with_config(indoc::indoc! {r"
    default_install_hook_types:
      - commit-msg
    repos:
      - repo: builtin
        hooks:
        - id: check-json
    "});
    context.git().add_all();

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/commit-msg`

    ----- stderr -----
    "#);

    let mut commit = context.git().command();
    commit.arg("commit").arg("-m").arg("dummy");

    cmd_snapshot!(context, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] dummy
     1 file changed, 6 insertions(+)
     create mode 100644 .pre-commit-config.yaml

    ----- stderr -----
    check json...........................................(no files to check)Skipped
    ");
}

#[test]
fn workspace_hook_impl_subdirectory() {
    let context = TestEnv::new_git().with_filter("[a-f0-9]{7}", "abc1234");
    let cwd = context.work_dir();

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

    context.write_workspace(["project2", "project3"], config);
    context.git().add_all();

    // Install from a subdirectory
    cmd_snapshot!(context, context.install().current_dir(cwd.join("project2")), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `../.git/hooks/pre-commit` for workspace `[TEMP_DIR]/project2`

    hint: this hook installed for `[TEMP_DIR]/project2` only; run `prek install` from `[TEMP_DIR]/` to install for the entire repo.

    ----- stderr -----
    "#);

    let mut commit = context.git().command();
    commit
        .arg("commit")
        .arg("-m")
        .arg("Test commit from subdirectory");

    cmd_snapshot!(context, commit, @r"
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
}

/// Install from a subdirectory, and run commit in another worktree.
#[test]
fn workspace_hook_impl_worktree_subdirectory() {
    let context = TestEnv::new_git().with_filter("[a-f0-9]{7}", "abc1234");
    let cwd = context.work_dir();

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

    context.write_workspace(["project2", "project3"], config);
    context.git().add_all().commit("Initial commit");

    // Install from a subdirectory
    cmd_snapshot!(context, context.install().current_dir(cwd.join("project2")), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `../.git/hooks/pre-commit` for workspace `[TEMP_DIR]/project2`

    hint: this hook installed for `[TEMP_DIR]/project2` only; run `prek install` from `[TEMP_DIR]/` to install for the entire repo.

    ----- stderr -----
    "#);

    // Create a new worktree.
    context.git().run(["worktree", "add", "worktree", "HEAD"]);

    // Modify the config in the main worktree
    context.write_file("project2/.pre-commit-config.yaml", "");

    let mut commit = context.git_at(context.child("worktree")).command();
    commit
        .arg("commit")
        .arg("-m")
        .arg("Test commit from subdirectory")
        .arg("--allow-empty");

    cmd_snapshot!(context, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [detached HEAD abc1234] Test commit from subdirectory

    ----- stderr -----
    Running in workspace: `[TEMP_DIR]/worktree/project2`
    Test Hook............................................(no files to check)Skipped
    ");
}

#[test]
fn workspace_hook_impl_no_project_found() {
    let context = TestEnv::new_git()
        .with_filter("[a-f0-9]{7}", "1d5e501")
        .with_file("empty/file.txt", "Some content");

    // Create a directory without .pre-commit-config.yaml
    let empty_dir = context.child("empty");
    context.git().add_all();

    // Install hook that allows missing config
    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    // Try to run hook-impl from directory without config
    let mut commit = context.git_at(&empty_dir).command();
    commit.arg("commit").arg("-m").arg("Test commit");

    cmd_snapshot!(context, commit, @r"
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
    let mut commit = context.git_at(&empty_dir).command();
    commit
        .env(EnvVars::PREK_ALLOW_NO_CONFIG, "1")
        .arg("commit")
        .arg("-m")
        .arg("Test commit");

    // The hook should simply succeed because there is no config
    cmd_snapshot!(context, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) 1d5e501] Test commit
     1 file changed, 1 insertion(+)
     create mode 100644 empty/file.txt

    ----- stderr -----
    ");

    // Create the root `.pre-commit-config.yaml`
    context.write_file(
        PRE_COMMIT_CONFIG_YAML,
        indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: fail
                name: fail
                entry: fail
                language: fail
    "},
    );
    context.git().add_all();

    // Commit with `PREK_ALLOW_NO_CONFIG=1` again, the hooks should run (and fail)
    let mut commit = context.git_at(&empty_dir).command();
    commit
        .env(EnvVars::PREK_ALLOW_NO_CONFIG, "1")
        .arg("commit")
        .arg("-m")
        .arg("Test commit");

    cmd_snapshot!(context, commit, @r"
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
}

#[test]
fn hook_impl_does_not_fail_when_no_hooks_match_stage() {
    let context = TestEnv::new_git()
        .with_filter("[a-f0-9]{7}", "abc1234")
        .with_file(
            PRE_COMMIT_CONFIG_YAML,
            indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: manual-only
                name: manual-only
                language: system
                entry: echo manual-only
                stages: [ manual ]
    "},
        )
        .with_file("file.txt", "x");

    // Only a manual-stage hook; a pre-commit hook run should find nothing for the stage.

    context.git().add_all();

    // Install the git hook (which invokes `prek hook-impl`).
    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    // Commit should succeed; the hook should not error just because no hooks match pre-commit.
    let mut commit = context.git().command();
    commit.arg("commit").arg("-m").arg("Test commit");

    cmd_snapshot!(context, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] Test commit
     2 files changed, 9 insertions(+)
     create mode 100644 .pre-commit-config.yaml
     create mode 100644 file.txt

    ----- stderr -----
    ");
}

#[test]
fn workspace_hook_impl_with_selectors() {
    let context = TestEnv::new_git().with_filter("[a-f0-9]{7}", "abc1234");
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

    context.write_workspace(["project2", "project3"], config);
    context.git().add_all();

    cmd_snapshot!(context, context.install().arg("project2/"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    let mut commit = context.git().command();
    commit
        .arg("commit")
        .arg("-m")
        .arg("Test commit from subdirectory");

    cmd_snapshot!(context, commit, @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [master (root-commit) abc1234] Test commit from subdirectory
     3 files changed, 24 insertions(+)
     create mode 100644 .pre-commit-config.yaml
     create mode 100644 project2/.pre-commit-config.yaml
     create mode 100644 project3/.pre-commit-config.yaml

    ----- stderr -----
    ✓ project2
      Test Hook..............................................................Passed
      - hook id: test-hook
      - duration: [TIME]

        cwd: [TEMP_DIR]/project2
    "#);
}
