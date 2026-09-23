use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use insta::assert_snapshot;
use prek_consts::PRE_COMMIT_CONFIG_YAML;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn run_in_non_git_repo() {
    let context = TestEnv::new();

    cmd_snapshot!(context, context.run().env(EnvVars::LC_ALL, "fr_FR.UTF-8"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Not in a Git repository. Change to a Git repository, or run `git init` to create one.
    "#);
}

#[test]
fn run_preserves_git_discovery_errors() {
    let context = TestEnv::new()
        .with_config("repos: []")
        .with_filter(
            r"Command `[^`]*git(?:\.exe)? rev-parse --absolute-git-dir --git-common-dir --git-path hooks --show-toplevel`",
            "Command `[GIT] rev-parse --absolute-git-dir --git-common-dir --git-path hooks --show-toplevel`",
        )
        .init_git()
        .with_file("invalid.gitconfig", "[invalid\n");

    cmd_snapshot!(context, context.run().env("GIT_CONFIG_GLOBAL", context.child("invalid.gitconfig").path()), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Command `[GIT] rev-parse --absolute-git-dir --git-common-dir --git-path hooks --show-toplevel` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: bad config line 1 in file [TEMP_DIR]/invalid.gitconfig
    "#);
    cmd_snapshot!(context, context.run().env(EnvVars::GIT_DIR, "missing"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Command `[GIT] rev-parse --absolute-git-dir --git-common-dir --git-path hooks --show-toplevel` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: not a git repository: 'missing'
    "#);
}

#[test]
fn staged_files_only() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'print(open("file.txt", "rt").read())'
                verbose: true
                types: [text]
       "#})
        .with_file("file.txt", "Hello, world!")
        .init_git();

    // Non-staged files should be stashed and restored.
    context.write_file("file.txt", "Hello world again!");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    trailing-whitespace......................................................Passed
    - hook id: trailing-whitespace
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    Unstaged changes detected. Temporarily saving them to `[HOME]/patches/[TIME]-[PID].patch`
    Restored unstaged changes from `[HOME]/patches/[TIME]-[PID].patch`
    ");

    let content = context.read("file.txt");
    assert_snapshot!(content, @"Hello world again!");
}

#[test]
fn intent_to_add_file_survives_conflicted_stash_restore() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: rewrite-python
                name: rewrite-python
                language: system
                entry: python3 -c 'open("test.py", "w").write("a = 1\n")'
                files: ^test\.py$
       "#})
        .init_git();

    context.git().add(PRE_COMMIT_CONFIG_YAML);

    context.write_file("intent.txt", "preserve me\n");
    context
        .git()
        .command()
        .arg("add")
        .arg("--intent-to-add")
        .arg("intent.txt")
        .assert()
        .success();

    context.write_file("test.py", "a=1\n");
    context.git().add("test.py");
    context.write_file("test.py", "a=1\nb = 2\n");

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    rewrite-python...........................................................Failed
    - hook id: rewrite-python
    - files were modified by this hook

    ----- stderr -----
    Unstaged changes detected. Temporarily saving them to `[HOME]/patches/[TIME]-[PID].patch`
    Hook changes conflicted with the saved unstaged changes. Reverting the hook changes
    Restored unstaged changes from `[HOME]/patches/[TIME]-[PID].patch`
    "#);

    assert_eq!(context.read("intent.txt"), "preserve me\n");
    assert_eq!(context.read("test.py"), "a=1\nb = 2\n");

    let output = context
        .git()
        .command()
        .arg("diff")
        .arg("--diff-filter=A")
        .arg("--name-only")
        .arg("--")
        .arg("intent.txt")
        .output()?;
    assert!(output.status.success(), "{output:?}");
    assert_eq!(String::from_utf8(output.stdout)?, "intent.txt\n");

    Ok(())
}

#[cfg(unix)]
#[test]
fn restore_on_interrupt() -> Result<()> {
    // The hook will sleep for 3 seconds.
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import time; open("out.txt", "wt").write(open("file.txt", "rt").read()); time.sleep(10)'
                verbose: true
                types: [text]
   "#})
        .with_file("file.txt", "Hello, world!")
        .init_git();

    // Non-staged files should be stashed and restored.
    context.write_file("file.txt", "Hello world again!");

    let mut child = context.run().spawn()?;
    let child_id = child.id();

    // Send an interrupt signal to the process.
    let handle = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(1));
        #[allow(clippy::cast_possible_wrap)]
        unsafe {
            libc::kill(child_id as i32, libc::SIGINT)
        };
    });

    handle.join().unwrap();
    child.wait()?;

    let content = context.read("out.txt");
    assert_snapshot!(content, @"Hello, world!");

    let content = context.read("file.txt");
    assert_snapshot!(content, @"Hello world again!");

    Ok(())
}

/// When in merge conflict, runs on files that have conflicts fixed.
#[test]
fn merge_conflicts() {
    let context = TestEnv::new()
        .with_file("file.txt", "Hello, world!")
        .init_git();

    // Create a merge conflict.
    context.git().commit("Initial commit");

    context.git().branch("feature").checkout("feature");
    context.write_file("file.txt", "Hello, world again!");
    context.git().add(".").commit("Feature commit");

    context.git().checkout("master");
    context.write_file("file.txt", "Hello, world from master!");
    context.git().add(".").commit("Master commit");

    context
        .git()
        .command()
        .arg("merge")
        .arg("feature")
        .assert()
        .code(1);

    context.write_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'import sys; print(sorted(sys.argv[1:]))'
                verbose: true
    "});

    // Abort on merge conflicts.
    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Found unresolved merge conflicts. Resolve the conflicts, stage the files with `git add`, and try again
    "#);

    // Fix the conflict and run again.
    context.git().add(".");
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    trailing-whitespace......................................................Passed
    - hook id: trailing-whitespace
    - duration: [TIME]

      ['.pre-commit-config.yaml', 'file.txt']

    ----- stderr -----
    ");
}

#[test]
fn run_last_commit() {
    // file2 starts with issues but is intentionally absent from the last commit.
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: trailing-whitespace
              - id: end-of-file-fixer
    "})
        .with_file("file1.txt", "Hello, world!\n")
        .with_file("file2.txt", "Initial content with trailing spaces   \n")
        .init_git();

    context.git().commit("Initial commit");

    // Modify files and make second commit with trailing whitespace
    context.write_file("file1.txt", "Hello, world!   \n"); // trailing whitespace
    context.write_file("file3.txt", "New file"); // missing newline
    // Note: file2.txt is NOT modified in this commit, so it should be filtered out by --last-commit
    context.git().add(".").commit("Second commit with issues");

    // Run with --last-commit should only check files from the last commit
    // This should only process file1.txt and file3.txt, NOT file2.txt
    cmd_snapshot!(context, context.run().arg("--last-commit"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - description: trims trailing whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing file1.txt
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - description: ensures that a file is either empty, or ends with one newline
    - exit code: 1
    - files were modified by this hook

      Fixing file3.txt

    ----- stderr -----
    ");

    // Now reset the files to their problematic state for comparison
    context.write_file("file1.txt", "Hello, world!   \n"); // trailing whitespace
    context.write_file("file3.txt", "New file"); // missing newline

    // Run with --all-files should check ALL files including file2.txt
    // This demonstrates that file2.txt was indeed filtered out in the previous test
    cmd_snapshot!(context, context.run().arg("--all-files"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - description: trims trailing whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing file1.txt
      Fixing file2.txt
    fix end of files.........................................................Failed
    - hook id: end-of-file-fixer
    - description: ensures that a file is either empty, or ends with one newline
    - exit code: 1
    - files were modified by this hook

      Fixing file3.txt

    ----- stderr -----
    ");
}

/// Test `git commit -a` works without `.git/index.lock exists` error.
#[test]
fn git_commit_a() {
    let context = TestEnv::new()
        .with_filter("7c8398204bbc95c33a6d2543f86a27621647cf78", "[HASH]")
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: echo
                name: echo
                language: system
                entry: echo
                verbose: true
    "})
        .with_file("file.txt", "Hello, world!\n")
        .init_git();

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    context.git().add(".").commit("Initial commit");

    // Edit the file
    context.write_file("file.txt", "Hello, world again!\n");

    let mut commit = context.git().command();
    commit.arg("commit").arg("-a").arg("-m").arg("Update file");

    cmd_snapshot!(context, commit, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [master COMMIT] Update file
     1 file changed, 1 insertion(+), 1 deletion(-)

    ----- stderr -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]

      file.txt
    ");
}

#[cfg(unix)]
#[test]
fn git_commit_a_currently_fails_when_hook_writes_to_temp_git_index() {
    // Repro for #1786 documenting the current behavior. `git commit -a`
    // exports `GIT_INDEX_FILE=.git/index.lock` to the hook process. If the
    // hook inherits that env var and then runs a git command that writes to an
    // index in a different repository, Git writes those entries into the
    // parent repo's temporary index instead.
    //
    // The important detail is that the temp repo stages `file.txt`, matching a tracked
    // path in the parent repo. `prek` treats the post-hook diff as a best-effort
    // snapshot, so the commit continues until Git tries to build trees from the
    // corrupted temporary index and fails with `invalid object ... for 'file.txt'`.
    let context = TestEnv::new()
        .with_filter(
            r"invalid object 100644 [0-9a-f]{40}",
            "invalid object 100644 [HASH]",
        )
        .with_file(
            "hook.sh",
            indoc::indoc! {r#"
        set -eu
        tmpdir="$(mktemp -d)"
        trap 'rm -rf "$tmpdir"' EXIT
        cd "$tmpdir"
        git init >/dev/null 2>&1
        printf 'hook version\n' > file.txt
        git add file.txt
    "#},
        )
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: write-temp-index
                name: write-temp-index
                language: system
                entry: sh hook.sh
                pass_filenames: false
                always_run: true
                verbose: true
    "})
        .with_file("file.txt", "Hello, world!\n")
        .init_git();

    cmd_snapshot!(context, context.install(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    context.git().add(".").commit("Initial commit");

    // `git commit` does not set `GIT_INDEX_FILE`; `git commit -a` does.
    // The repro only triggers on the `-a` path.
    context.write_file("file.txt", "Hello again!\n");

    let mut commit = context.git().command();
    commit.arg("commit").arg("-a").arg("-m").arg("Update file");

    cmd_snapshot!(context, commit, @r"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    write-temp-index.........................................................Passed
    - hook id: write-temp-index
    - duration: [TIME]
    error: invalid object 100644 [HASH] for 'file.txt'
    error: Error building trees
    "
    );
}

#[test]
fn run_with_tree_object_as_ref() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: echo-files
                name: echo files
                entry: echo
                language: system
                pass_filenames: true
    "})
        .with_file("file1.txt", "hello")
        .init_git();

    context.git().commit("Initial commit");

    // Create some changes and stage them
    context.write_file("file2.txt", "world");
    context.git().add("file2.txt");

    // Get the tree object from the staged changes
    let tree_output = context
        .git()
        .command()
        .arg("write-tree")
        .output()
        .expect("Failed to run git write-tree");
    let tree_sha = String::from_utf8_lossy(&tree_output.stdout)
        .trim()
        .to_string();

    // Run prek with tree object as to-ref (should work with .. syntax)
    cmd_snapshot!(context, context.run()
        .arg("--from-ref").arg("HEAD")
        .arg("--to-ref").arg(&tree_sha), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    echo files...............................................................Passed

    ----- stderr -----
    ");
}
