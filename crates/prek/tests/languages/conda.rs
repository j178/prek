use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn language_version() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    let context = context.with_filter(r"OpenSSL [^\n]+", "OpenSSL [VERSION]");

    cmd_snapshot!(context, context.run(), @r"
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
fn remote_repo_install() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "conda-hook",
            indoc::indoc! {r"
            - id: conda-remote
              name: conda-remote
              language: conda
              entry: openssl version
        "},
        )
        .with_file(
            "environment.yml",
            indoc::indoc! {r"
            channels:
              - conda-forge
            dependencies:
              - openssl
        "},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: conda-remote
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo});

    context.git().add(".");

    let context = context.with_filter(r"OpenSSL [^\n]+", "OpenSSL [VERSION]");

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    conda-remote.............................................................Passed
    - hook id: conda-remote
    - duration: [TIME]

      OpenSSL [VERSION]

    ----- stderr -----
    ");
}
