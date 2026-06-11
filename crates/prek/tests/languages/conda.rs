use assert_fs::fixture::{FileWriteStr, PathChild};
use prek_consts::PRE_COMMIT_HOOKS_YAML;

use crate::common::{TestContext, cmd_snapshot};

#[test]
fn language_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: conda-version
                name: conda-version
                language: conda
                entry: openssl version
                language_version: '3.12'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `conda-version`
      caused by: Hook specified `language_version: 3.12` but the language `conda` does not support toolchain installation for now
    ");
}

#[test]
fn local_hook_with_additional_dependencies() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: conda-local
                name: conda-local
                language: conda
                entry: openssl version
                additional_dependencies: [openssl]
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    let mut filters = context.filters();
    filters.push((r"OpenSSL [^\n]+", "OpenSSL [VERSION]"));

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    conda-local..............................................................Passed
    - hook id: conda-local
    - duration: [TIME]

      OpenSSL [VERSION]

    ----- stderr -----
    ");
}

#[test]
fn remote_repo_install() -> anyhow::Result<()> {
    let hook_repo = TestContext::new();
    hook_repo.init_project();

    hook_repo
        .work_dir()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
            - id: conda-remote
              name: conda-remote
              language: conda
              entry: openssl version
        "})?;

    hook_repo
        .work_dir()
        .child("environment.yml")
        .write_str(indoc::indoc! {r"
            channels:
              - conda-forge
            dependencies:
              - openssl
        "})?;

    hook_repo.git_add(".");
    hook_repo.git_commit("Add conda hook");
    hook_repo.git_tag("v1.0.0");

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: conda-remote
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo.work_dir().display()});

    context.git_add(".");

    let mut filters = context.filters();
    filters.push((r"OpenSSL [^\n]+", "OpenSSL [VERSION]"));

    cmd_snapshot!(filters, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    conda-remote.............................................................Passed
    - hook id: conda-remote
    - duration: [TIME]

      OpenSSL [VERSION]

    ----- stderr -----
    ");

    Ok(())
}
