mod common;

use crate::common::{TestContext, cmd_snapshot};

use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};

#[test]
fn check_useless_excludes() -> anyhow::Result<()> {
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
    cmd_snapshot!(context.filters(), context.run().arg("check-useless-excludes"), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    Check useless excludes...................................................Failed
    - hook id: check-useless-excludes
    - exit code: 1
      The exclude pattern `^useless/$` for `echo` does not match any files

    ----- stderr -----
    "#);

    Ok(())
}
