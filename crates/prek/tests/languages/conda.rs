use prek_consts::env_vars::EnvVars;

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
fn local_pixi_environment_is_reused_after_installer_changes() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: conda-local-pixi
                name: conda-local-pixi
                language: conda
                entry: openssl version
                additional_dependencies: [openssl]
                always_run: true
                verbose: true
                pass_filenames: false
    "})
        .with_file(
            "pixi-config.toml",
            "default-channels = [\"conda-forge\"]\ndetached-environments = true\n",
        )
        .init_git();

    let pixi_config = context.work_dir().join("pixi-config.toml");
    let context = context.with_filter(r"OpenSSL [^\n]+", "OpenSSL [VERSION]");
    let mut command = context.run();
    command
        .env(EnvVars::PREK_CONDA_INSTALLER, "pixi")
        .env("PIXI_CONFIG_FILE", pixi_config);

    cmd_snapshot!(context, command, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    conda-local-pixi.........................................................Passed
    - hook id: conda-local-pixi
    - duration: [TIME]

      OpenSSL [VERSION]

    ----- stderr -----
    ");

    let mut command = context.run();
    command.env(EnvVars::PREK_CONDA_INSTALLER, "micromamba");

    cmd_snapshot!(context, command, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    conda-local-pixi.........................................................Passed
    - hook id: conda-local-pixi
    - duration: [TIME]

      OpenSSL [VERSION]

    ----- stderr -----
    ");

    assert_eq!(
        fs_err::read_dir(context.home_dir().join("hooks"))
            .unwrap()
            .count(),
        1,
    );
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

#[test]
fn remote_repo_install_using_pixi() {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "conda-pixi-hook",
            indoc::indoc! {r"
            - id: conda-remote-pixi
              name: conda-remote-pixi
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
              - id: conda-remote-pixi
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo});

    context.git().add(".");

    let context = context.with_filter(r"OpenSSL [^\n]+", "OpenSSL [VERSION]");
    let mut command = context.run();
    command.env(EnvVars::PREK_CONDA_INSTALLER, "pixi");

    cmd_snapshot!(context, command, @r"
    success: true
    exit_code: 0
    ----- stdout -----
    conda-remote-pixi........................................................Passed
    - hook id: conda-remote-pixi
    - duration: [TIME]

      OpenSSL [VERSION]

    ----- stderr -----
    ");
}
