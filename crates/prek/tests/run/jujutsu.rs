use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot, jj_cmd};

/// A secondary (non-colocated) jj workspace has no `.git` directory at all; prek must
/// still resolve the backing Git store and select the working-copy changeset.
#[test]
fn run_in_non_colocated_jj_workspace() {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print('ARGS:' + ' '.join(sys.argv[1:]))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    context.write_file("file.txt", "hello");
    context.write_file("ignored.md", "nope");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ARGS:file.txt

    ----- stderr -----
    ");
}

/// In a colocated jj workspace the default `prek run` mode must select the working-copy
/// changeset. Git's index is empty (jj does not stage), so the old staged-files path
/// would run on nothing.
#[test]
fn run_default_in_colocated_jj_workspace() {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print('ARGS:' + ' '.join(sys.argv[1:]))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    context.write_file("file.txt", "hello");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ARGS:file.txt

    ----- stderr -----
    ");
}

/// `--all-files` in a jj workspace must list every tracked file via the jj backend.
#[test]
fn run_all_files_in_jj_workspace() {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print('ARGS:' + ' '.join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    context.write_file("a.txt", "a");
    context.write_file("b.txt", "b");

    // `jj file list` auto-snapshots the working copy, so newly written files are
    // tracked without an explicit commit.
    cmd_snapshot!(context, context.run().arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ARGS:a.txt b.txt

    ----- stderr -----
    ");
}

/// When the prek workspace root is a subdirectory of the jj workspace root, jj
/// reports repo-root-relative paths that must be stripped to project-relative
/// ones. Regression test for running with the config in a nested directory.
#[test]
fn run_in_nested_jj_workspace() {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    let context = context.with_project_config(
        "project",
        indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print(chr(10).join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#},
    );
    context.write_file("project/app.txt", "in project");
    context.write_file("root.txt", "outside");

    cmd_snapshot!(context, context.run().current_dir(context.child("project").path()), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      app.txt

    ----- stderr -----
    ");
}

/// Deleted files must not be passed to hooks in a jj workspace, matching git's
/// `--diff-filter` which drops deletions.
#[test]
fn run_excludes_deleted_files_in_jj_workspace() -> Result<()> {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return Ok(());
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print(chr(10).join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    context.write_file("keep.txt", "keep");
    context.write_file("del.txt", "gone");

    // Commit the baseline so the deletion below is a real change against the parent.
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    fs_err::remove_file(context.child("del.txt").path())?;
    context.write_file("keep.txt", "changed");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      keep.txt

    ----- stderr -----
    ");

    Ok(())
}

/// The default run must select files with unresolved conflicts in a jj workspace, and
/// they must be reported exactly: a long path or one containing spaces cannot be read
/// back from `jj resolve --list`'s display table, because a path of 35 characters or
/// more is separated from its description by a single space.
#[test]
fn run_selects_conflicted_files_in_jj_workspace() {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    // Two conflicted paths: one longer than the table's column, one with spaces.
    let long = "subdirectory/with/a/very/long/file-name-here.txt";
    let spaced = "weird name.txt";

    let jj = |args: &[&str]| {
        jj_cmd(context.work_dir())
            .unwrap()
            .args(args)
            .assert()
            .success();
    };

    // Build a real conflict: a base revision and two divergent edits, then merge.
    context.write_file(long, "line1\nline2\nline3\n");
    context.write_file(spaced, "line1\nline2\nline3\n");
    jj(&["describe", "-m", "base"]);
    jj(&["bookmark", "create", "base", "-r", "@"]);
    jj(&["new", "-m", "x"]);
    context.write_file(long, "line1\nXXX\nline3\n");
    context.write_file(spaced, "line1\nXXX\nline3\n");
    jj(&["bookmark", "create", "x", "-r", "@"]);
    jj(&["new", "base", "-m", "y"]);
    context.write_file(long, "line1\nYYY\nline3\n");
    context.write_file(spaced, "line1\nYYY\nline3\n");
    jj(&["bookmark", "create", "y", "-r", "@"]);
    jj(&["new", "x", "y", "-m", "merge"]);
    // An ordinary working-copy edit next to the conflicts: jj keeps both, so the run must
    // select both, unlike Git where a merge state replaces the selection.
    context.write_file("extra.txt", "edit\n");

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print(chr(10).join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      extra.txt
      subdirectory/with/a/very/long/file-name-here.txt
      weird name.txt

    ----- stderr -----
    ");
}

/// `--from-ref`/`--to-ref` in a jj workspace selects files changed between two
/// revisions using merge-base (`from...to`) semantics, matching the Git backend.
/// With divergent refs, edits made only on the `from` side must NOT be selected.
#[test]
fn run_from_ref_to_ref_in_jj_workspace() {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print(chr(10).join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    let jj = |args: &[&str]| {
        jj_cmd(context.work_dir())
            .unwrap()
            .args(args)
            .assert()
            .success();
    };

    // base, then two divergent branches from it.
    context.write_file("base.txt", "base");
    jj(&["describe", "-m", "base"]);
    jj(&["bookmark", "create", "base", "-r", "@"]);
    // `from` branch adds a file that only exists on its side.
    jj(&["new", "-m", "from"]);
    context.write_file("from_only.txt", "from");
    jj(&["bookmark", "create", "from", "-r", "@"]);
    // `to` branch diverges from base with its own file.
    jj(&["new", "base", "-m", "to"]);
    context.write_file("to_only.txt", "to");
    jj(&["bookmark", "create", "to", "-r", "@"]);

    // Merge-base semantics: only to_only.txt is in `from...to`. A direct from->to
    // diff would also report from_only.txt (as a deletion), which must not happen.
    cmd_snapshot!(context, context.run().arg("--from-ref").arg("from").arg("--to-ref").arg("to"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      to_only.txt

    ----- stderr -----
    ");
}

/// A renamed file must be selected by its real (new) path. `jj diff --types` renders
/// renames with git-style `{a => b}` compaction, which would produce a bogus path
/// that names no file and silently bypass hooks; the diff template avoids that.
#[test]
fn run_selects_renamed_file_in_jj_workspace() -> Result<()> {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return Ok(());
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print(chr(10).join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    context.write_file("orig.txt", "content\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    // Rename the tracked file in the working copy.
    fs_err::rename(
        context.child("orig.txt").path(),
        context.child("renamed.txt").path(),
    )?;

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      renamed.txt

    ----- stderr -----
    ");

    Ok(())
}

/// `--last-commit` selects the commit that was just completed, not the empty working-copy
/// commit that `jj commit` leaves behind.
#[test]
fn run_last_commit_in_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print('ARGS:' + ' '.join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_file("committed.txt", "hello\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    cmd_snapshot!(context, context.run().arg("--last-commit"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ARGS:committed.txt

    ----- stderr -----
    ");
}

/// The hook's own Git defaults are what a Jujutsu workspace falls back to when the completed
/// commit has no parent: its parent is jj's root, whose all-zero ID Git rejects in a diff range,
/// so prek exports no pair and the hook runs `git diff --cached` instead.
#[test]
fn run_last_commit_omits_git_refs_in_non_colocated_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: forbid-new-submodules
                types: [text]
    "});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    context.write_file("committed.txt", "hello\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    cmd_snapshot!(context, context.run().arg("--last-commit"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    forbid new submodules....................................................Passed

    ----- stderr -----
    ");
}

/// Outside that first-commit case, `--last-commit` exports the pair as commit IDs from the
/// backing store, so a hook that runs Git on them resolves the completed commit and its parent
/// even where the store's `HEAD` is unborn.
#[test]
fn run_last_commit_provides_git_resolvable_refs_in_jj_workspace() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: refs
                name: refs
                language: system
                entry: python3 refs.py
                files: \.txt$
                verbose: true
                pass_filenames: false
    "})
        .with_file(
            "refs.py",
            indoc::indoc! {r#"
        import os
        import subprocess

        def subject(ref):
            return subprocess.run(
                ["git", "log", "-1", "--format=%s", ref],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

        for name in ("PRE_COMMIT_FROM_REF", "PRE_COMMIT_TO_REF"):
            ref = os.environ.get(name)
            print(f"{name}: {subject(ref) if ref else 'unset'}")
    "#},
        );

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    context.write_file("committed.txt", "one\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();
    context.write_file("committed.txt", "two\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "second"])
        .assert()
        .success();

    cmd_snapshot!(context, context.run().arg("--last-commit"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    refs.....................................................................Passed
    - hook id: refs
    - duration: [TIME]

      PRE_COMMIT_FROM_REF: baseline
      PRE_COMMIT_TO_REF: second

    ----- stderr -----
    ");
}

/// A revset naming several commits has no single ID to hand Git, and the revset itself is not a
/// revision Git can resolve, so no pair is exported rather than a run of concatenated IDs. In a
/// merge working copy `@-` is exactly that: both parents.
#[test]
fn run_omits_git_refs_for_a_multi_commit_revset_in_jj_workspace() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: refs
                name: refs
                language: system
                entry: python3 refs.py
                files: \.txt$
                verbose: true
                pass_filenames: false
    "})
        .with_file(
            "refs.py",
            indoc::indoc! {r#"
        import os
        import subprocess

        def subject(ref):
            return subprocess.run(
                ["git", "log", "-1", "--format=%s", ref],
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

        for name in ("PRE_COMMIT_FROM_REF", "PRE_COMMIT_TO_REF"):
            ref = os.environ.get(name)
            print(f"{name}: {subject(ref) if ref else 'unset'}")
    "#},
        );

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    let jj = |args: &[&str]| {
        jj_cmd(context.work_dir())
            .unwrap()
            .args(args)
            .assert()
            .success();
    };

    context.write_file("base.txt", "base\n");
    jj(&["describe", "-m", "base"]);
    jj(&["bookmark", "create", "base", "-r", "@"]);
    jj(&["new", "-m", "first"]);
    context.write_file("first.txt", "first\n");
    jj(&["bookmark", "create", "first", "-r", "@"]);
    jj(&["new", "base", "-m", "second"]);
    context.write_file("second.txt", "second\n");
    jj(&["bookmark", "create", "second", "-r", "@"]);
    // The working copy is the merge itself, so `@-` is both of its parents.
    jj(&["new", "first", "second", "-m", "merge"]);

    cmd_snapshot!(context, context.run().arg("--from-ref").arg("@-"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    refs.....................................................................Passed
    - hook id: refs
    - duration: [TIME]

      PRE_COMMIT_FROM_REF: unset
      PRE_COMMIT_TO_REF: unset

    ----- stderr -----
    ");
}

/// A Git repository nested inside a jj workspace still gets its branch checked: the backend comes
/// from the hook's own directory, not from where prek started, and the workspace's injected Git
/// environment must not leak into it.
#[test]
fn run_checks_a_nested_git_repository_branch_in_jj_workspace() {
    let context = TestEnv::new().with_config("repos: []").with_project_config(
        "nested",
        indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
    "},
    );

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    // On the harness's default branch, `master`, which the hook protects.
    fs_err::create_dir_all(context.child("nested")).unwrap();
    context.git_at(context.child("nested")).init();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    × nested
      don't commit to branch.................................................Failed
      - hook id: no-commit-to-branch
      - description: Protects specific branches from direct commits
      - exit code: 1

        You are not allowed to commit to branch 'master'

    ----- stderr -----
    ");
}

/// A jj workspace driven through a backing store has no Git repository for Git to find, so the
/// builtin branch check skips rather than blocking every run.
#[test]
fn run_skips_the_branch_check_in_non_colocated_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
    "});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    context.write_file("test.txt", "hello\n");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");
}

/// A jj workspace nested inside a Git checkout is still a jj workspace: the nearest boundary
/// wins, so the branch check skips instead of reading the outer checkout's branch.
#[test]
fn run_skips_the_branch_check_for_a_jj_workspace_inside_a_git_checkout() -> Result<()> {
    let context = TestEnv::new().with_config("repos: []").init_git();
    context.git().commit("Initial commit");

    let work = context.child("inner");
    fs_err::create_dir_all(&work)?;
    let Some(mut init) = jj_cmd(&work) else {
        return Ok(());
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();
    context.write_file(
        "inner/.pre-commit-config.yaml",
        indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
    "},
    );

    cmd_snapshot!(context, context.run().current_dir(&work), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    don't commit to branch...................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

/// A hook that rewrites a file in a Git repository nested inside a jj workspace has to be caught:
/// the workspace's changeset does not cover that repository, so the sample has to come from Git
/// there.
#[test]
fn run_reports_hook_modifications_in_a_nested_git_repository() -> Result<()> {
    let context = TestEnv::new()
        .with_config("repos: []")
        .with_project_config(
            "nested",
            indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: rewrite
                name: rewrite
                language: system
                # Runs without filenames: jj does not track the contents of a nested Git
                # repository, so no file inside one is ever selected.
                always_run: true
                pass_filenames: false
                entry: python3 -c "from pathlib import Path; Path('committed.txt').write_text('MODIFIED\n')"
    "#},
        )
        // Git abbreviates blob hashes, and the length is not fixed.
        .with_filter(r"index \w+\.\.\w+ \d{6}", "index [OLD]..[NEW] 100644");

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return Ok(());
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    fs_err::create_dir_all(context.child("nested"))?;
    context.git_at(context.child("nested")).init();
    context.write_file("nested/committed.txt", "original\n");
    context
        .git_at(context.child("nested"))
        .add("committed.txt")
        .commit("Add file");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    × nested
      rewrite................................................................Failed
      - hook id: rewrite
      - files were modified by this hook

    ----- stderr -----
    ");

    // The failure patch has to cover the repository the hook changed, not just the workspace.
    context.write_file("nested/committed.txt", "original\n");
    cmd_snapshot!(context, context.run()
        .env_remove(EnvVars::CI)
        .arg("--show-diff-on-failure"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    × nested
      rewrite................................................................Failed
      - hook id: rewrite
      - files were modified by this hook
    All changes made by hooks:
    diff --git a/.pre-commit-config.yaml b/.pre-commit-config.yaml
    new file mode 100644
    index 0000000000..a62c20941d
    --- /dev/null
    +++ b/.pre-commit-config.yaml
    @@ -0,0 +1,1 @@
    +repos: []
    \ No newline at end of file
    diff --git a/committed.txt b/committed.txt
    index [OLD]..[NEW] 100644
    --- a/committed.txt
    +++ b/committed.txt
    @@ -1 +1 @@
    -original
    +MODIFIED

    ----- stderr -----
    ");

    Ok(())
}

/// A checkout named by `GIT_DIR`/`GIT_WORK_TREE` is the repository, even when prek is started
/// inside a Jujutsu workspace: the branch check has to come from there rather than skip as it does
/// for the workspace.
#[test]
fn run_uses_the_checkout_selected_by_git_dir_in_jj_workspace() -> Result<()> {
    let context = TestEnv::new().with_config("repos: []");
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return Ok(());
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    // Another checkout, selected through the environment, on a protected branch of its own.
    fs_err::create_dir_all(context.child("checkout"))?;
    context.git_at(context.child("checkout")).init();
    context.write_file(
        "checkout/.pre-commit-config.yaml",
        indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: no-commit-to-branch
    "},
    );
    context
        .git_at(context.child("checkout"))
        .add(".")
        .commit("Add hook");

    let checkout = context.child("checkout").path().to_path_buf();
    cmd_snapshot!(context, context.command()
        .arg("run")
        .arg("--config")
        .arg(checkout.join(".pre-commit-config.yaml"))
        .env(EnvVars::GIT_DIR, checkout.join(".git"))
        .env(EnvVars::GIT_WORK_TREE, &checkout), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    don't commit to branch...................................................Failed
    - hook id: no-commit-to-branch
    - description: Protects specific branches from direct commits
    - exit code: 1

      You are not allowed to commit to branch 'master'

    ----- stderr -----
    ");

    Ok(())
}

/// A pair jj declines is not inherited from a parent process: a stale `PRE_COMMIT_FROM_REF`
/// would have `forbid-new-submodules` diff an unrelated range instead of `--cached`.
#[test]
fn run_clears_inherited_refs_when_jj_declines_them() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: refs
                name: refs
                language: system
                entry: python3 refs.py
                verbose: true
                always_run: true
                pass_filenames: false
    "})
        .with_file(
            "refs.py",
            indoc::indoc! {r#"
        import os

        for name in ("PRE_COMMIT_FROM_REF", "PRE_COMMIT_TO_REF", "PRE_COMMIT_ORIGIN", "PRE_COMMIT_SOURCE"):
            print(f"{name}: {os.environ.get(name, 'unset')}")
    "#},
        );

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return Ok(());
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    context.write_file("committed.txt", "one\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    cmd_snapshot!(context, context.run()
        .env("PRE_COMMIT_FROM_REF", "parent-from")
        .env("PRE_COMMIT_TO_REF", "parent-to")
        .env("PRE_COMMIT_ORIGIN", "parent-from")
        .env("PRE_COMMIT_SOURCE", "parent-to")
        .arg("--last-commit"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    refs.....................................................................Passed
    - hook id: refs
    - duration: [TIME]

      PRE_COMMIT_FROM_REF: unset
      PRE_COMMIT_TO_REF: unset
      PRE_COMMIT_ORIGIN: unset
      PRE_COMMIT_SOURCE: unset

    ----- stderr -----
    ");

    // A pre-push of a root commit names only the newer side, and jj declines that lone ref as
    // well: the inherited pair must be cleared all the same.
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: refs
                name: refs
                language: system
                entry: python3 refs.py
                stages: [pre-push]
                verbose: true
                always_run: true
                pass_filenames: false
    "});

    let head = String::from_utf8(
        jj_cmd(context.work_dir())
            .unwrap()
            .args(["log", "-r", "@-", "--no-graph", "-T", "commit_id"])
            .output()?
            .stdout,
    )?;
    let stdin = format!(
        "refs/heads/new {} refs/heads/new {}\n",
        head.trim(),
        "0".repeat(40)
    );

    cmd_snapshot!(context, context.command()
        .args(["hook-impl", "--hook-type", "pre-push", "--", "origin", "unused"])
        .env("PRE_COMMIT_FROM_REF", "parent-from")
        .env("PRE_COMMIT_TO_REF", "parent-to")
        .env("PRE_COMMIT_ORIGIN", "parent-from")
        .env("PRE_COMMIT_SOURCE", "parent-to")
        .pass_stdin(stdin), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    refs.....................................................................Passed
    - hook id: refs
    - duration: [TIME]

      PRE_COMMIT_FROM_REF: unset
      PRE_COMMIT_TO_REF: unset
      PRE_COMMIT_ORIGIN: unset
      PRE_COMMIT_SOURCE: unset

    ----- stderr -----
    ");

    Ok(())
}

/// A filename with a newline in it has to reach the hook whole: file records are NUL-terminated,
/// so a name is never split in two. The hook prints the name's bytes in hex, which nothing in
/// the snapshot filters can rewrite. Windows rejects control characters in file names, so this
/// only runs where they are allowed.
#[cfg(unix)]
#[test]
fn run_preserves_a_newline_in_a_filename_in_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                language: system
                entry: python3 -c "import os, sys; print([os.fsencode(f).hex() for f in sys.argv[1:]])"
                files: \.txt$
                verbose: true
    "#});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_file("new\nline.txt", "hello\n");

    // `6e65770a6c696e652e747874` is `new\nline.txt`. Both the working-copy changeset and the
    // tracked-file listing have to keep the name whole.
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ['6e65770a6c696e652e747874']

    ----- stderr -----
    ");
    cmd_snapshot!(context, context.run().arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ['6e65770a6c696e652e747874']

    ----- stderr -----
    ");
}

/// A working-copy path that is not valid UTF-8 does not reach file selection at all, so a hook
/// never receives it: jj reports paths as text, and the earlier newline case is the one this
/// change has to keep whole. A filesystem may refuse such a name (macOS requires valid UTF-8),
/// and then there is nothing to check either way.
#[cfg(unix)]
#[test]
fn run_does_not_select_a_non_utf8_filename_in_jj_workspace() {
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt as _;

    let name = std::path::Path::new(OsStr::from_bytes(b"invalid\xff.txt"));
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                language: system
                # Prints the name's bytes if it ever arrives.
                entry: python3 -c "import os, sys; print([os.fsencode(f).hex() for f in sys.argv[1:]])"
                files: \.txt$
                verbose: true
    "#});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    if fs_err::write(context.child(name), "hello\n").is_err() {
        return;
    }

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...........................................(no files to check)Skipped

    ----- stderr -----
    ");
}

/// Explicit refs are translated individually: `--from-ref HEAD~1` (with `--to-ref`
/// defaulting to `HEAD`) covers the working-copy changeset, unlike `--last-commit`, which
/// selects the commit that was just completed.
#[test]
fn run_from_head_parent_in_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print('ARGS:' + ' '.join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_file("committed.txt", "hello\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    // Only the working copy changes after the commit.
    context.write_file("working.txt", "edit\n");

    cmd_snapshot!(context, context.run().arg("--from-ref").arg("HEAD~1"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ARGS:working.txt

    ----- stderr -----
    ");
}

/// `--last-commit` after a merge diffs against the first parent, like Git's `HEAD~1`,
/// rather than against the parents' common ancestor.
#[test]
fn run_last_commit_after_merge_in_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print('ARGS:' + ' '.join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    let jj = |args: &[&str]| {
        jj_cmd(context.work_dir())
            .unwrap()
            .args(args)
            .assert()
            .success();
    };

    context.write_file("base.txt", "base\n");
    jj(&["describe", "-m", "base"]);
    jj(&["bookmark", "create", "base", "-r", "@"]);
    jj(&["new", "-m", "first"]);
    context.write_file("first.txt", "first\n");
    jj(&["bookmark", "create", "first", "-r", "@"]);
    jj(&["new", "base", "-m", "second"]);
    context.write_file("second.txt", "second\n");
    jj(&["bookmark", "create", "second", "-r", "@"]);
    jj(&["new", "first", "second", "-m", "merge"]);
    jj(&["commit", "-m", "merge"]);

    cmd_snapshot!(context, context.run().arg("--last-commit"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ARGS:second.txt

    ----- stderr -----
    ");
}

/// An inherited `GIT_DIR` that does not resolve is reported rather than quietly replaced
/// by the workspace's backing store.
#[test]
fn run_reports_broken_git_dir_in_colocated_jj_workspace() {
    let context = TestEnv::new().with_config("repos: []").with_filter(
        r"Command `[^`]*git(?:\.exe)? rev-parse --absolute-git-dir --git-common-dir --git-path hooks --show-toplevel`",
        "Command `[GIT] rev-parse --absolute-git-dir --git-common-dir --git-path hooks --show-toplevel`",
    );

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_file("file.txt", "hello\n");

    cmd_snapshot!(context, context.run().env("GIT_DIR", "missing"), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Command `[GIT] rev-parse --absolute-git-dir --git-common-dir --git-path hooks --show-toplevel` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: not a git repository: 'missing'
    ");
}

/// A styled jj configuration must not leak ANSI sequences into the paths prek parses.
///
/// With `ui.color = "always"` jj wraps its whole listing, so the first path it prints
/// carries an escape prefix. prek cannot resolve such a path, and silently skips it,
/// which is why the fixture file is named to sort first.
#[test]
fn run_ignores_jj_colors_in_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print('ARGS:' + ' '.join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    let styled = context.child("jj-styled.toml");
    fs_err::write(
        styled.path(),
        "ui.color = \"always\"\n[colors]\nfile_list = \"red\"\ndiff = \"red\"\n",
    )
    .unwrap();

    context.write_file(".a.txt", "a");

    cmd_snapshot!(context, context.run().env("JJ_CONFIG", styled.path()).arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ARGS:.a.txt

    ----- stderr -----
    ");
}

/// A hook's own Git commands must find the repository, which a workspace with no
/// `.git` of its own cannot do unassisted: prek hands the hook the backing store.
#[test]
fn run_provides_git_env_to_hooks_in_non_colocated_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: git-in-hook
                name: git-in-hook
                language: system
                entry: python3 -c "import subprocess; subprocess.run(['git', 'rev-parse', '--show-toplevel'], check=True)"
                files: \.txt$
    "#});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    context.write_file("file.txt", "hello\n");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    git-in-hook..............................................................Passed

    ----- stderr -----
    ");
}

/// With `include_deleted`, a rename must also hand the hook the removed source path,
/// matching the Git behavior covered by `include_deleted.rs`.
#[test]
fn run_selects_renamed_source_in_jj_workspace() -> Result<()> {
    let context = TestEnv::new().with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: existing
                name: existing
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                types: [text]
                verbose: true
              - id: python
                name: python
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:])'
                include_deleted: true
                types: [python]
                verbose: true
    "});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return Ok(());
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_file("script.py", "print('hello')\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    fs_err::rename(
        context.child("script.py").path(),
        context.child("script.txt").path(),
    )?;

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    existing.................................................................Passed
    - hook id: existing
    - duration: [TIME]

      ['script.txt']
    python...................................................................Passed
    - hook id: python
    - duration: [TIME]

      ['script.py']

    ----- stderr -----
    ");

    Ok(())
}

/// A Git repository strictly inside a Jujutsu workspace is the nearer boundary, and is
/// chosen before the workspace's own metadata is read: unreadable metadata must not hide
/// the checkout the user selected.
#[test]
#[cfg(unix)]
fn run_prefers_nested_git_repository_over_broken_jj_metadata() {
    let context = TestEnv::new().with_project_config(
        "nested",
        indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print('ARGS:' + ' '.join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#},
    );

    // An enclosing Jujutsu workspace whose backing store cannot be read: a symlink loop.
    fs_err::create_dir_all(context.child(".jj/repo/store")).unwrap();
    let looped = context.child(".jj/repo/store/git_target");
    std::os::unix::fs::symlink(looped.path(), looped.path()).unwrap();

    context.write_file("nested/keep.txt", "x\n");
    context
        .git_at(context.child("nested"))
        .init()
        .add("keep.txt");

    cmd_snapshot!(context, context
        .run()
        .current_dir(context.child("nested").path())
        .arg("--all-files"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      ARGS:keep.txt

    ----- stderr -----
    ");
}

/// `.jj` metadata with no usable Git store is reported, rather than silently hooking an
/// unrelated enclosing Git repository.
#[test]
fn run_reports_jujutsu_workspace_without_git_store() {
    let context = TestEnv::new().with_config("repos: []");
    // The layout a non-Git Jujutsu backend leaves behind: metadata, but no Git store.
    fs_err::create_dir_all(context.child(".jj/repo/store")).unwrap();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Detected a Jujutsu workspace at `[TEMP_DIR]/`, but prek could not find the Git repository backing it
    ");
}

/// A nested Git repository inside a workspace with no `.git` of its own keeps its own
/// repository for hook commands: prek must not hand them the outer backing store.
#[test]
fn run_keeps_nested_git_repository_for_hooks_in_non_colocated_jj_workspace() {
    let context = TestEnv::new()
        .with_config("repos: []")
        .with_project_config(
            "nested",
            indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: git-in-hook
                name: git-in-hook
                language: system
                # Runs without filenames: jj does not track the contents of a nested
                # Git repository, so no file inside one is ever selected.
                always_run: true
                pass_filenames: false
                entry: python3 -c "import pathlib, subprocess, sys; out = subprocess.run(['git', 'rev-parse', '--show-toplevel'], capture_output=True, text=True, check=True).stdout.strip(); sys.exit(0 if pathlib.Path(out).name == 'nested' else 1)"
    "#},
        );

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    // A nested Git repository, which the hook's own Git commands must address.
    fs_err::create_dir_all(context.child("nested")).unwrap();
    context.git_at(context.child("nested")).init();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ nested
      git-in-hook............................................................Passed

    ----- stderr -----
    ");
}

/// A hook that rewrites a file must be reported as a failure in a colocated jj
/// workspace, and `--show-diff-on-failure` must print the rewrite. jj snapshots the
/// rewritten file into Git's index, so a Git diff of the same tree comes back empty; the
/// patch has to come from the backend that noticed the change.
#[test]
fn run_reports_hook_modifications_in_colocated_jj_workspace() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rewrite
                name: rewrite
                language: system
                entry: python3 rewrite.py
                files: \.txt$
    "})
        .with_file(
            "rewrite.py",
            indoc::indoc! {r"
        from pathlib import Path
        import sys

        for filename in sys.argv[1:]:
            Path(filename).write_text('MODIFIED\n')
    "},
        )
        // jj abbreviates blob hashes longer than Git's seven characters.
        .with_filter(r"index \w+\.\.\w+ \d{6}", "index [OLD]..[NEW] 100644");

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_file("file.txt", "original\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();
    // A working-copy change, so the default run has a file to hand to the hook.
    context.write_file("file.txt", "changed\n");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    rewrite..................................................................Failed
    - hook id: rewrite
    - files were modified by this hook

    ----- stderr -----
    ");

    // Same run again, now rendering the patch it detected.
    context.write_file("file.txt", "changed\n");
    cmd_snapshot!(context, context.run().env_remove(EnvVars::CI).arg("--show-diff-on-failure"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    rewrite..................................................................Failed
    - hook id: rewrite
    - files were modified by this hook
    All changes made by hooks:
    diff --git a/file.txt b/file.txt
    index [OLD]..[NEW] 100644
    --- a/file.txt
    +++ b/file.txt
    @@ -1,1 +1,1 @@
    -original
    +MODIFIED

    ----- stderr -----
    ");
}

/// `--show-diff-on-failure` prints the changes of the project being run, not of the whole
/// Jujutsu workspace: the diff runs in the project directory, and without a fileset it would
/// cover every other project in the workspace.
#[test]
fn run_scopes_the_hook_modification_diff_to_the_project_in_jj_workspace() {
    let context = TestEnv::new();
    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    let context = context
        .with_project_config(
            "project",
            indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: rewrite
                name: rewrite
                language: system
                entry: python3 rewrite.py
                files: \.txt$
    "},
        )
        .with_file(
            "project/rewrite.py",
            indoc::indoc! {r"
        from pathlib import Path
        import sys

        for filename in sys.argv[1:]:
            Path(filename).write_text('MODIFIED\n')
    "},
        )
        // jj abbreviates blob hashes longer than Git's seven characters.
        .with_filter(r"index \w+\.\.\w+ \d{6}", "index [OLD]..[NEW] 100644");

    context.write_file("project/app.txt", "original\n");
    context.write_file("root.txt", "original\n");
    jj_cmd(context.work_dir())
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();
    // Both files are working-copy changes, so an unscoped diff would print both.
    context.write_file("project/app.txt", "changed\n");
    context.write_file("root.txt", "changed\n");

    cmd_snapshot!(context, context.run()
        .current_dir(context.child("project").path())
        .env_remove(EnvVars::CI)
        .arg("--show-diff-on-failure"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    rewrite..................................................................Failed
    - hook id: rewrite
    - files were modified by this hook
    All changes made by hooks:
    diff --git a/project/app.txt b/project/app.txt
    index [OLD]..[NEW] 100644
    --- a/project/app.txt
    +++ b/project/app.txt
    @@ -1,1 +1,1 @@
    -original
    +MODIFIED

    ----- stderr -----
    ");
}

/// Hooks that only read the repository must not look like they modified it: a jj query
/// snapshots the working copy, and in a colocated workspace that snapshot rewrites the
/// Git index, which changes a Git diff of the same tree.
#[test]
fn run_ignores_snapshotting_in_colocated_jj_workspace() {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: builtin
            hooks:
              - id: check-case-conflict
                priority: 1
          - repo: local
            hooks:
              - id: readonly
                name: readonly
                language: system
                entry: "true"
                pass_filenames: false
                priority: 1
    "#});

    let Some(mut init) = jj_cmd(context.work_dir()) else {
        return;
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    // Untracked when the run starts, so only the snapshot taken while it runs turns it
    // into a change Git can see.
    context.write_file("New.txt", "new\n");

    cmd_snapshot!(context, context.run().arg("--files").arg("New.txt"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check for case conflicts.................................................Passed
    readonly.................................................................Passed

    ----- stderr -----
    ");
}

/// End-to-end coverage for a secondary workspace (`jj workspace add`) with no `.git`
/// of its own: the backing Git directory must be resolved from the main workspace, so
/// `prek run` works with no `.git` present at all.
///
/// The main workspace is initialized without colocation because a colocated one makes
/// jj create a Git worktree for the new workspace (see the colocated case below).
#[test]
fn run_in_secondary_jj_workspace() -> Result<()> {
    let context = TestEnv::new();
    let main = context.work_dir();
    let Some(mut init) = jj_cmd(main) else {
        return Ok(());
    };
    init.args(["git", "init", "--no-colocate"])
        .assert()
        .success();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print(chr(10).join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});
    // Commit so the config is on an ancestor the secondary workspace checks out.
    jj_cmd(main)
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    // Add a secondary workspace as a sibling directory (no `.git` of its own).
    let secondary = main.path().parent().unwrap().join(format!(
        "{}-secondary",
        main.path().file_name().unwrap().to_string_lossy()
    ));
    jj_cmd(main)
        .unwrap()
        .args(["workspace", "add", &secondary.to_string_lossy()])
        .assert()
        .success();
    assert!(secondary.join(".jj").exists());
    assert!(!secondary.join(".git").exists());

    fs_err::write(secondary.join("second.txt"), "hi\n")?;

    cmd_snapshot!(context, context.run().current_dir(&secondary), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      second.txt

    ----- stderr -----
    ");

    Ok(())
}

/// A secondary workspace of a *colocated* repository has a `.git` entry (a Git worktree
/// link, on jj versions that create one). prek still detects the Jujutsu workspace, so
/// the default run selects the working-copy changeset rather than Git's staged files.
#[test]
fn run_in_colocated_secondary_jj_workspace() -> Result<()> {
    let context = TestEnv::new();
    let main = context.work_dir();
    let Some(mut init) = jj_cmd(main) else {
        return Ok(());
    };
    init.args(["git", "init", "--colocate"]).assert().success();

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo-files
                entry: python3 -c "import sys; print(chr(10).join(sorted(sys.argv[1:])))"
                language: system
                files: \.txt$
                verbose: true
    "#});
    jj_cmd(main)
        .unwrap()
        .args(["commit", "-m", "baseline"])
        .assert()
        .success();

    let secondary = main.path().parent().unwrap().join(format!(
        "{}-colocated-secondary",
        main.path().file_name().unwrap().to_string_lossy()
    ));
    jj_cmd(main)
        .unwrap()
        .args(["workspace", "add", &secondary.to_string_lossy()])
        .assert()
        .success();
    assert!(secondary.join(".jj").exists());

    fs_err::write(secondary.join("second.txt"), "hi\n")?;

    cmd_snapshot!(context, context.run().current_dir(&secondary), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo-files...............................................................Passed
    - hook id: echo-files
    - duration: [TIME]

      second.txt

    ----- stderr -----
    ");

    Ok(())
}
