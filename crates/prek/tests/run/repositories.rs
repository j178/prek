use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;

use crate::common::{TestEnv, cmd_snapshot};

fn with_remote_fetch_error_filters(context: TestEnv) -> TestEnv {
    context.with_filters([
        (
            r"Command `[^`]*git(?:\.exe)? fetch origin --tags`",
            "Command `[GIT] fetch origin --tags`",
        ),
        (
            r"fatal: unable to access 'https://notexistentatallnevergonnahappen\.com/nonexistent/repo/':.*",
            r"fatal: unable to access 'https://notexistentatallnevergonnahappen.com/nonexistent/repo/': [error]",
        ),
    ])
}

/// Use same repo multiple times, with same or different revisions.
#[test]
fn same_repo() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: trailing-whitespace
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: trailing-whitespace
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v4.6.0
            hooks:
              - id: trailing-whitespace
    "})
        .with_files([
            ("file.txt", "Hello, world!\n"),
            ("valid.json", "{}"),
            ("invalid.json", "{}"),
            ("main.py", r#"print "abc"  "#),
        ])
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    trim trailing whitespace.................................................Failed
    - hook id: trailing-whitespace
    - description: trims trailing whitespace
    - exit code: 1
    - files were modified by this hook

      Fixing main.py
    trim trailing whitespace.................................................Passed
    trim trailing whitespace.................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn hook_repo_placeholder_expands_to_local_project() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: hook-repo-local
                name: local
                language: system
                entry: git -C "{hook_repo}" cat-file -e HEAD:hook-repo-marker
                always_run: true
                pass_filenames: false
    "#})
        .with_file("hook-repo-marker", "")
        .init_git();

    context.git().commit("Add local hook");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    local....................................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn hook_repo_placeholder_expands_to_remote_checkout() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "hook-repo-placeholder",
            indoc::indoc! {r#"
            - id: hook-repo-remote
              name: remote
              language: system
              entry: git -C "{hook_repo}" cat-file -e HEAD:hook-repo-marker
              always_run: true
              pass_filenames: false
        "#},
        )
        .with_file("hook-repo-marker", "")
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: '{}'
            rev: v1.0.0
            hooks:
              - id: hook-repo-remote
    ", hook_repo});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    remote...................................................................Passed

    ----- stderr -----
    "#);
}

/// Initialize a repo that does not exist.
#[test]
fn init_nonexistent_repo() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: https://notexistentatallnevergonnahappen.com/nonexistent/repo
            rev: v1.0.0
            hooks:
              - id: nonexistent
                name: nonexistent
        "})
        .init_git();

    let context = with_remote_fetch_error_filters(context);

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Failed to clone repo `https://notexistentatallnevergonnahappen.com/nonexistent/repo`
      caused by: Command `[GIT] fetch origin --tags` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: unable to access 'https://notexistentatallnevergonnahappen.com/nonexistent/repo/': [error]
    "#);
}

#[test]
fn skipped_remote_repo_is_not_cloned() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer

          - repo: https://notexistentatallnevergonnahappen.com/nonexistent/repo
            rev: v1.0.0
            hooks:
              - id: ruff-check
        "})
        .init_git();

    cmd_snapshot!(context,
        context.run().arg("--all-files").arg("--skip").arg("ruff-check"),
        @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fix end of files.........................................................Passed

    ----- stderr -----
    "
    );
}

#[test]
fn skipped_same_key_remote_repo_entry_is_not_initialized() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "duplicate-key-hook",
            indoc::indoc! {r"
        - id: test-hook
          name: Test Hook
          entry: echo ok
          language: system
          always_run: true
    "},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {repo}
            rev: v1.0.0
            hooks:
              - id: missing-hook

          - repo: {repo}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo = hook_repo});
    context.git().add(".");

    context
        .run()
        .arg("--all-files")
        .arg("--skip")
        .arg("missing-hook")
        .assert()
        .success();
}

#[test]
fn required_group_excluded_remote_repo_is_not_cloned() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer
                groups: [ci, fast]

          - repo: https://notexistentatallnevergonnahappen.com/nonexistent/repo
            rev: v1.0.0
            hooks:
              - id: ruff-check
                groups: [ci]
        "})
        .init_git();

    cmd_snapshot!(context,
        context
            .run()
            .arg("--all-files")
            .arg("--require-group")
            .arg("ci")
            .arg("--require-group")
            .arg("fast"),
        @r"
    success: true
    exit_code: 0
    ----- stdout -----
    fix end of files.........................................................Passed

    ----- stderr -----
    "
    );
}

#[test]
fn unmatched_skip_does_not_suppress_remote_clone() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: builtin
            hooks:
              - id: end-of-file-fixer

          - repo: https://notexistentatallnevergonnahappen.com/nonexistent/repo
            rev: v1.0.0
            hooks:
              - id: ruff-check
        "})
        .init_git();

    let context = with_remote_fetch_error_filters(context);

    cmd_snapshot!(
        context,
        context.run().arg("--all-files").arg("--skip").arg("other-hook"),
        @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Failed to clone repo `https://notexistentatallnevergonnahappen.com/nonexistent/repo`
      caused by: Command `[GIT] fetch origin --tags` exited with an error:

    [status]
    exit status: 128

    [stderr]
    fatal: unable to access 'https://notexistentatallnevergonnahappen.com/nonexistent/repo/': [error]
    "#
    );
}

/// Test reusing hook environments only when dependencies are exactly same. (ignore order)
#[test]
fn reuse_env() -> Result<()> {
    let context = TestEnv::new()
        .with_file(
            "local_pkg/setup.py",
            indoc::indoc! {r#"
        from setuptools import setup

        setup(
            name="local-pkg",
            version="0.1.0",
            py_modules=["local_pkg"],
        )
    "#},
        )
        .with_file(
            "local_pkg/local_pkg.py",
            "def hello():\n     print('hello')\n",
        )
        .init_git();
    let pkg_dir = context.child("local_pkg");

    let dependency = serde_json::to_string(&std::path::absolute(pkg_dir.path())?)?;
    context.write_config(indoc::formatdoc! {r#"
    repos:
      - repo: local
        hooks:
          - id: reuse-env
            name: reuse-env
            language: python
            entry: python -c "import local_pkg; local_pkg.hello()"
            pass_filenames: false
            additional_dependencies: [{dependency}]
            verbose: true
    "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    reuse-env................................................................Passed
    - hook id: reuse-env
    - duration: [TIME]

      hello

    ----- stderr -----
    ");

    // Remove dependencies, so the environment should not be reused.
    context.write_config(indoc::indoc! {r#"
    repos:
      - repo: local
        hooks:
          - id: reuse-env
            name: reuse-env
            language: python
            entry: python -c "print('ok')"
            pass_filenames: false
            verbose: true
    "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    reuse-env................................................................Passed
    - hook id: reuse-env
    - duration: [TIME]

      ok

    ----- stderr -----
    ");

    // There should be two hook environments.
    assert_eq!(context.home_dir().child("hooks").read_dir()?.count(), 2);

    Ok(())
}
