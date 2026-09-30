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

#[test]
fn restaging_hook_preserves_conflicted_stash() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: restage
                name: restage
                language: system
                entry: python3 hook.py
                files: ^doc\.md$
                pass_filenames: false
        "})
        .with_file(
            "hook.py",
            indoc::indoc! {r"
                from pathlib import Path
                import subprocess

                Path('doc.md').write_bytes(b'NORMALISED\nbody\nstaged edit\n')
                Path('hook-added.txt').write_bytes(b'hook output\n')
                subprocess.run(['git', 'add', '.'], check=True)
                subprocess.run(['git', 'rm', '-q', 'removed.txt'], check=True)
            "},
        )
        .with_file("doc.md", "ORIGINAL\nbody\n")
        .with_file("other.txt", "other original\n")
        .with_file("removed.txt", "keep this file\n")
        .init_git();
    context.git().commit("Initial commit");
    context.write_file("doc.md", "ORIGINAL\nbody\nstaged edit\n");
    context.git().add("doc.md");
    context.write_file("doc.md", "ORIGINAL\nbody\nstaged edit\nUNSTAGED TAIL\n");
    context.write_file("other.txt", "other original\nother unstaged\n");
    context.write_file("intent.txt", "intent to add\n");
    context.git().run(["add", "--intent-to-add", "intent.txt"]);
    context.write_file("untracked.txt", "keep untracked content\n");
    context.command().arg("install").assert().success();

    cmd_snapshot!(context, context.git().command().env_remove("RUST_LOG").args(["commit", "-m", "Must not commit rolled-back fixes"]), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    Unstaged changes detected. Temporarily saving them to `[HOME]/patches/[TIME]-[PID].patch`
    restage..................................................................Passed
    Hook changes conflicted with the saved unstaged changes. Reverting the hook changes
    Restored unstaged changes from `[HOME]/patches/[TIME]-[PID].patch`
    "#);
    cmd_snapshot!(context, context.git().command().args(["status", "--short"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    MM doc.md
     A intent.txt
     M other.txt
    ?? hook-added.txt
    ?? untracked.txt

    ----- stderr -----
    "#);
    cmd_snapshot!(context, context.git().command().args(["show", ":doc.md"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ORIGINAL
    body
    staged edit

    ----- stderr -----
    "#);
    assert_eq!(
        context.read("doc.md"),
        "ORIGINAL\nbody\nstaged edit\nUNSTAGED TAIL\n"
    );
    assert_eq!(
        context.read("other.txt"),
        "other original\nother unstaged\n"
    );
    assert_eq!(context.read("removed.txt"), "keep this file\n");
    assert_eq!(context.read("intent.txt"), "intent to add\n");
    assert_eq!(context.read("hook-added.txt"), "hook output\n");
    assert_eq!(context.read("untracked.txt"), "keep untracked content\n");
}

#[test]
fn failed_stash_restore_aborts_commit_and_allows_recovery() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: restage
                name: restage
                language: system
                entry: python3 hook.py
                files: ^doc\.md$
                pass_filenames: false
        "})
        .with_file(
            "hook.py",
            indoc::indoc! {r"
                from pathlib import Path
                import subprocess

                Path('doc.md').write_bytes(b'NORMALISED\nbody\nstaged edit\n')
                subprocess.run(['git', 'add', 'doc.md'], check=True)
                Path('.git/index.lock').touch()
            "},
        )
        .with_file("doc.md", "ORIGINAL\nbody\n")
        .with_file("other.txt", "other original\n")
        .with_filter(r"\b[a-f0-9]{40,64}\b", "[TREE]")
        .with_filter(r"Command `[^`]*git(?:\.exe)? ", "Command `[GIT] ")
        .with_filter(
            r"(?s)Another git process seems to be running in this repository.*",
            "[GIT_LOCK_HINT]",
        )
        .init_git();
    context.git().commit("Initial commit");
    context.write_file("doc.md", "ORIGINAL\nbody\nstaged edit\n");
    context.git().add("doc.md");
    context.write_file("doc.md", "ORIGINAL\nbody\nstaged edit\nUNSTAGED TAIL\n");
    context.write_file("other.txt", "other original\nother unstaged\n");
    context.command().arg("install").assert().success();

    cmd_snapshot!(context, context.git().command().env_remove("RUST_LOG").args(["commit", "-m", "Must not commit after failed restoration"]), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    Unstaged changes detected. Temporarily saving them to `[HOME]/patches/[TIME]-[PID].patch`
    restage..................................................................Passed
    Hook changes conflicted with the saved unstaged changes. Reverting the hook changes
    error: Failed to restore unstaged changes.
    Your changes are saved in `[HOME]/patches/[TIME]-[PID].patch`.
    Pre-hook index tree: [TREE]
      caused by: Failed to restore the pre-hook index
      caused by: Command `[GIT] reset --quiet [TREE] -- [TEMP_DIR]/` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: Unable to create '[TEMP_DIR]/.git/index.lock': File exists.
    [GIT_LOCK_HINT]
    "#);
    cmd_snapshot!(context, context.git().command().args(["log", "-1", "--format=%s"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Initial commit

    ----- stderr -----
    "#);

    // Keep the checkout locked to verify recovery uses an independent index.
    let patch = fs_err::read_dir(context.home_dir().join("patches"))?
        .next()
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("Missing recovery patch"))?
        .path();
    let patch_content = fs_err::read_to_string(&patch)?;
    let tree = patch_content
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("# prek pre-hook index tree: "))
        .ok_or_else(|| anyhow::anyhow!("Missing recovery tree"))?;
    let index = context.home_dir().join("recovery.index");
    let recovered = context.home_dir().join("recovered files");
    context
        .git()
        .command()
        .env("GIT_INDEX_FILE", &index)
        .arg("read-tree")
        .arg(tree)
        .assert()
        .success();
    context
        .git()
        .command()
        .env("GIT_INDEX_FILE", &index)
        .args(["checkout-index", "--all"])
        .arg(format!("--prefix={}/", recovered.display()))
        .assert()
        .success();
    assert_eq!(
        fs_err::read_to_string(recovered.join("doc.md"))?,
        "ORIGINAL\nbody\nstaged edit\n"
    );
    context
        .git_at(&recovered)
        .command()
        .args(["apply", "--check"])
        .arg(&patch)
        .assert()
        .success();
    context
        .git_at(&recovered)
        .command()
        .arg("apply")
        .arg(&patch)
        .assert()
        .success();
    assert_eq!(
        fs_err::read_to_string(recovered.join("doc.md"))?,
        "ORIGINAL\nbody\nstaged edit\nUNSTAGED TAIL\n"
    );
    assert_eq!(
        fs_err::read_to_string(recovered.join("other.txt"))?,
        "other original\nother unstaged\n"
    );
    assert_eq!(context.read("doc.md"), "NORMALISED\nbody\nstaged edit\n");
    Ok(())
}

#[test]
fn restore_intent_and_unstaged_changes_from_subdirectory() {
    let context = TestEnv::new()
        .with_file(
            format!("project/{PRE_COMMIT_CONFIG_YAML}"),
            indoc::indoc! {r#"
            repos:
              - repo: local
                hooks:
                  - id: check-staged
                    name: check staged
                    language: system
                    entry: python3 -c 'assert open("tracked.txt").read() == "staged\n"'
                    files: ^tracked\.txt$
        "#},
        )
        .with_file("project/tracked.txt", "staged\n")
        .with_file("project/nested/.gitkeep", "")
        .init_git();
    context.git().run(["config", "diff.relative", "true"]);
    context.write_file("project/tracked.txt", "unstaged\n");
    context.write_file("project/intent.txt", "intent\n");
    context.write_file("outside.txt", "outside\n");
    context.git().run(["add", "--intent-to-add", "."]);

    cmd_snapshot!(context, context.run().current_dir(context.child("project/nested")), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    check staged.............................................................Passed

    ----- stderr -----
    Unstaged changes detected. Temporarily saving them to `[HOME]/patches/[TIME]-[PID].patch`
    Restored unstaged changes from `[HOME]/patches/[TIME]-[PID].patch`
    "#);

    assert_eq!(context.read("project/tracked.txt"), "unstaged\n");
    assert_eq!(context.read("project/intent.txt"), "intent\n");
    assert_eq!(context.read("outside.txt"), "outside\n");
    cmd_snapshot!(context, context.git().command().args(["diff", "--name-only", "--diff-filter=A"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    outside.txt
    project/intent.txt

    ----- stderr -----
    "#);
}

#[test]
fn restore_after_checkout_failure() {
    let context = TestEnv::new()
        .with_filter(r"Command `[^`]*git(?:\.exe)? ", "Command `[GIT] ")
        .with_config(indoc::indoc! {r"
            repos:
              - repo: local
                hooks:
                  - id: check
                    name: check
                    language: system
                    entry: python3 -c 'pass'
        "})
        .with_file("file.txt", "original\n")
        .init_git();
    context.git().commit("Initial commit");
    context.write_file("file.txt", "staged\n");
    context.git().add("file.txt");
    context.write_file("file.txt", "unstaged\n");
    context.write_file("intent.txt", "intent\n");
    context.git().run(["add", "--intent-to-add", "intent.txt"]);
    context.write_executable_file(
        ".git/hooks/post-checkout",
        "#!/bin/sh\necho 'checkout hook failed' >&2\nexit 1\n",
    );

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    Unstaged changes detected. Temporarily saving them to `[HOME]/patches/[TIME]-[PID].patch`
    Restored unstaged changes from `[HOME]/patches/[TIME]-[PID].patch`
    error: Failed to clean work tree
      caused by: Failed to checkout working tree
      caused by: Command `[GIT] -c submodule.recurse=0 checkout -- [TEMP_DIR]/` exited with an error:

    [status]
    exit status: 1

    [stderr]
    checkout hook failed
    "#);
    assert_eq!(context.read("file.txt"), "unstaged\n");
    assert_eq!(context.read("intent.txt"), "intent\n");
    cmd_snapshot!(context, context.git().command().args(["status", "--porcelain"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    MM file.txt
     A intent.txt

    ----- stderr -----
    "#);
}

#[cfg(unix)]
#[test]
fn restore_when_interrupted_during_git_operations() -> Result<()> {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::process::Stdio;
    use std::time::{Duration, Instant};

    use prek_consts::env_vars::EnvVarsRead;

    for operation in ["rm", "write-tree", "checkout", "apply", "add"] {
        let context = TestEnv::new()
            .with_config(indoc::indoc! {r"
                repos:
                  - repo: local
                    hooks:
                      - id: check
                        name: check
                        language: system
                        entry: python3 -c 'pass'
            "})
            .with_file("file.txt", "original\n")
            .init_git();
        context.git().commit("Initial commit");
        context.write_file("file.txt", "staged\n");
        context.git().add("file.txt");
        context.write_file("file.txt", "unstaged\n");
        context.write_file("intent.txt", "intent\n");
        context.git().run(["add", "--intent-to-add", "intent.txt"]);
        context.write_executable_file(
            ".git/bin/git",
            indoc::indoc! {r"
                #!/usr/bin/env python3
                import os
                import socket
                import subprocess
                import sys

                args = sys.argv[1:]
                result = subprocess.run([os.environ['REAL_GIT'], *args])
                if 'apply' in args:
                    with open('.git/apply-calls', 'a') as calls:
                        calls.write('apply\n')
                if os.environ['INTERRUPT_OPERATION'] in args:
                    host, port = os.environ['INTERRUPT_ADDRESS'].split(':')
                    with socket.create_connection((host, int(port)), timeout=30) as sock:
                        sock.sendall(b'ready')
                        assert sock.recv(1) == b'x'
                sys.exit(result.returncode)
            "},
        );
        let listener = TcpListener::bind("127.0.0.1:0")?;
        listener.set_nonblocking(true)?;
        let path = std::env::join_paths(
            std::iter::once(context.child(".git/bin").path().to_path_buf()).chain(
                std::env::split_paths(&EnvVars.var_os(EnvVars::PATH).unwrap_or_default()),
            ),
        )?;
        let mut child = context
            .run()
            .env("PATH", path)
            .env("REAL_GIT", which::which("git")?)
            .env("INTERRUPT_OPERATION", operation)
            .env("INTERRUPT_ADDRESS", listener.local_addr()?.to_string())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;

        let deadline = Instant::now() + Duration::from_secs(30);
        let mut socket = loop {
            match listener.accept() {
                Ok((socket, _)) => break socket,
                Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(err) => return Err(err.into()),
            }
            if child.try_wait()?.is_some() || Instant::now() >= deadline {
                let _ = child.kill();
                let output = child.wait_with_output()?;
                anyhow::bail!(
                    "Git {operation} did not reach the barrier: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        // Accepted sockets inherit nonblocking mode on macOS.
        socket.set_nonblocking(false)?;
        socket.set_read_timeout(Some(Duration::from_secs(30)))?;
        let mut ready = [0; 5];
        socket.read_exact(&mut ready)?;
        let child_id = i32::try_from(child.id())?;
        // Git has changed the repository but has not returned to prek yet.
        let signaled = unsafe { libc::kill(child_id, libc::SIGINT) };
        anyhow::ensure!(
            signaled == 0,
            "Failed to interrupt Git {operation}: {}",
            std::io::Error::last_os_error()
        );
        socket.write_all(b"x")?;
        child
            .wait_with_output()?
            .assert()
            .append_context("operation", operation)
            .code(130);

        assert_eq!(context.read("file.txt"), "unstaged\n", "{operation}");
        assert_eq!(context.read("intent.txt"), "intent\n", "{operation}");
        assert_eq!(context.read(".git/apply-calls"), "apply\n", "{operation}");
        insta::allow_duplicates! {
        cmd_snapshot!(context, context.git().command().args(["status", "--porcelain"]), @r#"
        success: true
        exit_code: 0
        ----- stdout -----
        MM file.txt
         A intent.txt

        ----- stderr -----
        "#);
        }
    }
    Ok(())
}

#[test]
fn restore_reports_hook_and_recovery_errors() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
            repos:
              - repo: local
                hooks:
                  - id: modify
                    name: modify
                    language: system
                    entry: python3 -c "from pathlib import Path; Path('file.txt').write_bytes(b'hook\n'); Path('.git/index.lock').touch()"
                    pass_filenames: false
                    priority: 0
                  - id: invalid
                    name: invalid
                    language: system
                    entry: ''
                    priority: 1
        "#})
        .with_file("file.txt", "original\n")
        .with_filter(r"\b[a-f0-9]{40,64}\b", "[TREE]")
        .with_filter(r"Command `[^`]*git(?:\.exe)? ", "Command `[GIT] ")
        .with_filter(
            r"(?m)Another git process[^\n]*(?:\n(?:e\.g\.|make sure|persist|remove the file)[^\n]*)*",
            "[GIT_LOCK_HINT]",
        )
        .init_git();
    context.write_file("file.txt", "unstaged\n");
    context.write_file("intent.txt", "intent\n");
    context.git().run(["add", "--intent-to-add", "intent.txt"]);

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    Unstaged changes detected. Temporarily saving them to `[HOME]/patches/[TIME]-[PID].patch`
    Hook changes conflicted with the saved unstaged changes. Reverting the hook changes
    error: Failed to run hook `invalid`: Invalid hook `invalid`: Failed to parse entry: entry is empty

    Worktree restoration also failed:
    Failed to restore unstaged changes.
    Your changes are saved in `[HOME]/patches/[TIME]-[PID].patch`.
    Pre-hook index tree: [TREE]: Failed to restore the pre-hook index: Command `[GIT] reset --quiet [TREE] -- [TEMP_DIR]/` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: Unable to create '[TEMP_DIR]/.git/index.lock': File exists.
    [GIT_LOCK_HINT]


    Additionally:
    Failed to restore intent-to-add changes: Command `[GIT] add --intent-to-add -- [TEMP_DIR]/intent.txt` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: Unable to create '[TEMP_DIR]/.git/index.lock': File exists.
    [GIT_LOCK_HINT]
    "#);
}

#[cfg(unix)]
#[test]
fn restore_on_interrupt() -> Result<()> {
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
    // Wait for the hook to observe the cleaned worktree before interrupting it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !fs_err::read(context.child("out.txt")).is_ok_and(|contents| contents == b"Hello, world!")
    {
        anyhow::ensure!(
            child.try_wait()?.is_none(),
            "prek exited before the hook ran"
        );
        if std::time::Instant::now() >= deadline {
            child.kill()?;
            child.wait()?;
            anyhow::bail!("Timed out waiting for the hook to start");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let child_id = i32::try_from(child.id())?;
    unsafe {
        libc::kill(child_id, libc::SIGINT);
    }
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
