mod common;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn meta_hooks() {
    let context = TestEnv::new_git()
        .with_files([
            ("file.txt", "Hello, world!\n"),
            ("valid.json", "{}"),
            ("invalid.json", "{}"),
            ("main.py", r#"print "abc"  "#),
        ])
        .with_config(indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: check-hooks-apply
              - id: check-useless-excludes
              - id: identity
          - repo: local
            hooks:
              - id: match-no-files
                name: match no files
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                files: ^nonexistent$
              - id: useless-exclude
                name: useless exclude
                language: system
                entry: python3 -c 'import sys; sys.exit(0)'
                exclude: $nonexistent^
    "});
    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Check hooks apply........................................................Failed
    - hook id: check-hooks-apply
    - exit code: 1

      match-no-files does not apply to this repository
    Check useless excludes...................................................Failed
    - hook id: check-useless-excludes
    - exit code: 1

      The exclude pattern `regex: $nonexistent^` for `useless-exclude` does not match any files
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]

      .pre-commit-config.yaml
      file.txt
      main.py
      invalid.json
      valid.json
    match no files.......................................(no files to check)Skipped
    useless exclude..........................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn meta_hooks_unknown_hook() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: this-hook-does-not-exist
    "});
    context.git().add_all();

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: error: line 4 column 9: unknown meta hook id `this-hook-does-not-exist`
     --> <input>:4:9
      |
    2 |   - repo: meta
    3 |     hooks:
    4 |       - id: this-hook-does-not-exist
      |         ^ unknown meta hook id `this-hook-does-not-exist`
    ");
}

#[test]
fn check_useless_excludes_remote() {
    // When checking useless excludes, remote hooks are not actually cloned,
    // so hook options defined from HookManifest are not used.
    // If applied, "types_or: [python, pyi]" from black-pre-commit-mirror
    // will filter out html files first, so the excludes would not be useless, and the test would fail.
    let pre_commit_config = indoc::formatdoc! {r"
    repos:
      - repo: https://github.com/psf/black-pre-commit-mirror
        rev: 25.1.0
        hooks:
          - id: black
            exclude: '^html/'
      - repo: local
        hooks:
          - id: echo
            name: echo
            entry: echo 'echoing'
            language: system
            exclude: '^useless/$'
      - repo: meta
        hooks:
            - id: check-useless-excludes
    "};
    let context = TestEnv::new_git()
        .with_file("html/file1.html", "<!DOCTYPE html>")
        .with_config(&pre_commit_config);
    context.git().add_all();
    cmd_snapshot!(context, context.run().arg("check-useless-excludes"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Check useless excludes...................................................Failed
    - hook id: check-useless-excludes
    - exit code: 1

      The exclude pattern `regex: ^useless/$` for `echo` does not match any files

    ----- stderr -----
    ");
}

#[test]
fn meta_hooks_workspace() {
    let context = TestEnv::new_git()
        .with_project_config(
            "app",
            indoc::indoc! {r"
        repos:
          - repo: meta
            hooks:
              - id: check-hooks-apply
              - id: check-useless-excludes
              - id: identity
          - repo: local
            hooks:
              - id: match-no-files
                name: match no files
                language: system
                entry: python3 -c 'import sys; print(sys.argv[1:]); exit(1)'
                files: ^nonexistent$
              - id: useless-exclude
                name: useless exclude
                language: system
                entry: python3 -c 'import sys; sys.exit(0)'
                exclude: $nonexistent^
    "},
        )
        .with_files([
            ("app/file.txt", "Hello, world!\n"),
            ("app/valid.json", "{}"),
            ("app/invalid.json", "{x}"),
            ("app/main.py", r#"print "abc"  "#),
        ])
        .with_config("repos: []");
    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    × app
      Check hooks apply......................................................Failed
      - hook id: check-hooks-apply
      - exit code: 1

        match-no-files does not apply to this repository
      Check useless excludes.................................................Failed
      - hook id: check-useless-excludes
      - exit code: 1

        The exclude pattern `regex: $nonexistent^` for `useless-exclude` does not match any files
      identity...............................................................Passed
      - hook id: identity
      - duration: [TIME]

        .pre-commit-config.yaml
        file.txt
        main.py
        invalid.json
        valid.json
      match no files.....................................(no files to check)Skipped
      useless exclude........................................................Passed

    ----- stderr -----
    "#);
}

#[test]
fn check_useless_excludes_workspace_paths_are_project_relative() {
    // Workspace layout:
    // - Root project has no hooks.
    // - Nested project `app/` runs `check-useless-excludes`.
    //
    // Regression: in workspace mode, `files`/`exclude` matching must use paths *relative to the
    // nested project root* (so anchored patterns like `^...$` work as expected).
    // The two sentinel files keep the anchored excludes from being reported as useless.
    let context = TestEnv::new_git()
        .with_file(
            "app/.pre-commit-config.yaml",
            indoc::indoc! {r"
        exclude: '^global_excluded$'
        repos:
          - repo: meta
            hooks:
              - id: check-useless-excludes
          - repo: local
            hooks:
              - id: ok
                name: ok
                language: system
                entry: python3 -c 'import sys; sys.exit(0)'
                exclude: '^hook_excluded$'
        "},
        )
        .with_file("app/global_excluded", "ignored\n")
        .with_file("app/hook_excluded", "ignored\n")
        .with_config("repos: []");
    context.git().add_all();

    cmd_snapshot!(context, context.run().arg("check-useless-excludes"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    ✓ app
      Check useless excludes.................................................Passed

    ----- stderr -----
    "#);
}
