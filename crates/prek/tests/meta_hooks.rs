mod common;

use crate::common::{TestContext, cmd_snapshot};

use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_CONFIG_YAML;

#[test]
fn meta_hooks() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    cwd.child("file.txt").write_str("Hello, world!\n")?;
    cwd.child("valid.json").write_str("{}")?;
    cwd.child("invalid.json").write_str("{}")?;
    cwd.child("main.py").write_str(r#"print "abc"  "#)?;

    context.write_pre_commit_config(indoc::indoc! {r"
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
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
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

      The exclude pattern `$nonexistent^` for `useless-exclude` does not match any files
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]

      file.txt
      .pre-commit-config.yaml
      valid.json
      invalid.json
      main.py
    match no files.......................................(no files to check)Skipped
    useless exclude..........................................................Passed

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn check_useless_excludes_remote() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

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
    context.work_dir().child("html").create_dir_all()?;
    context
        .work_dir()
        .child("html")
        .child("file1.html")
        .write_str("<!DOCTYPE html>")?;

    context.write_pre_commit_config(&pre_commit_config);
    context.git_add(".");
    cmd_snapshot!(context.filters(), context.run().arg("check-useless-excludes"), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Check useless excludes...................................................Failed
    - hook id: check-useless-excludes
    - exit code: 1

      The exclude pattern `^useless/$` for `echo` does not match any files

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn meta_hooks_workspace() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let app = context.work_dir().child("app");
    app.create_dir_all()?;
    app.child(PRE_COMMIT_CONFIG_YAML)
        .write_str(indoc::indoc! {r"
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
    "})?;

    app.child("file.txt").write_str("Hello, world!\n")?;
    app.child("valid.json").write_str("{}")?;
    app.child("invalid.json").write_str("{x}")?;
    app.child("main.py").write_str(r#"print "abc"  "#)?;

    context.write_pre_commit_config("repos: []");
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    Running hooks for `app`:
    Check hooks apply........................................................Failed
    - hook id: check-hooks-apply
    - exit code: 1

      match-no-files does not apply to this repository
    Check useless excludes...................................................Failed
    - hook id: check-useless-excludes
    - exit code: 1

      The exclude pattern `$nonexistent^` for `useless-exclude` does not match any files
    identity.................................................................Passed
    - hook id: identity
    - duration: [TIME]

      file.txt
      .pre-commit-config.yaml
      valid.json
      invalid.json
      main.py
    match no files.......................................(no files to check)Skipped
    useless exclude..........................................................Passed

    ----- stderr -----
    ");

    Ok(())
}
