use std::env::consts::EXE_EXTENSION;

use anyhow::Result;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use crate::common::{TestEnv, cmd_snapshot, make_executable};

#[test]
fn reuses_managed_mise() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: mise-managed
                name: mise managed
                language: mise
                language_version: "=2026.7.18"
                entry: mise --version
                additional_dependencies: ["github:ajeetdsouza/zoxide@0.10.0"]
                always_run: true
                verbose: true
                pass_filenames: false
    "#})
        .init_git();

    let context = context.with_filter(
        r"2026\.7\.18 [^\r\n]+ \(\d{4}-\d{2}-\d{2}\)",
        "2026.7.18 [PLATFORM] ([DATE])",
    );

    cmd_snapshot!(context, context.run()
        .env(EnvVars::PREK_INTERNAL__MISE_BINARY_NAME, "mise-never-exists"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    mise managed.............................................................Passed
    - hook id: mise-managed
    - duration: [TIME]

      2026.7.18 [PLATFORM] ([DATE])

    ----- stderr -----
    "#);

    // A different environment requirement forces another installer call. With downloads disabled
    // and no system binary, this run can only reuse the managed mise installed above.
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: mise-managed
                name: mise managed
                language: mise
                language_version: system
                entry: mise --version
                always_run: true
                verbose: true
                pass_filenames: false
    "});
    context.git().add(".");

    cmd_snapshot!(context, context.run()
        .env(EnvVars::PREK_INTERNAL__MISE_BINARY_NAME, "mise-never-exists"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    mise managed.............................................................Passed
    - hook id: mise-managed
    - duration: [TIME]

      2026.7.18 [PLATFORM] ([DATE])

    ----- stderr -----
    "#);
}

#[test]
fn system_mise_installs_and_activates_dependencies() -> Result<()> {
    // Early miserc discovery must not read configuration from the calling project.
    let context = TestEnv::new()
        .with_file(".miserc.toml", "not valid = [")
        .init_git();

    let hook_repo = context
        .create_hook_repo(
            "mise-system-hook",
            indoc::indoc! {r#"
            - id: mise-system
              name: mise system
              language: mise
              language_version: system
              entry: zoxide --version
              additional_dependencies: ["github:ajeetdsouza/zoxide@0.10.0"]
              always_run: true
              verbose: true
              pass_filenames: false
        "#},
        )
        // Provisioning must not read configuration from the hook repository.
        .with_file("mise.toml", "not valid = [")
        .build();

    // Keep a conflicting executable beside the real system mise. Activating the private tool must
    // not move this whole directory ahead of the PATH returned by `mise env`.
    let system_bin = context.home_dir().child("system-bin");
    fs_err::create_dir_all(&system_bin)?;
    let system_mise = system_bin.join("mise").with_extension(EXE_EXTENSION);
    fs_err::copy(which::which("mise")?, &system_mise)?;
    make_executable(&system_mise)?;
    #[cfg(unix)]
    {
        let system_zoxide = system_bin.join("zoxide");
        fs_err::write(&system_zoxide, "#!/bin/sh\nexit 1\n")?;
        make_executable(&system_zoxide)?;
    }
    #[cfg(windows)]
    fs_err::write(system_bin.join("zoxide.cmd"), "@exit /b 1\r\n")?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: '{}'
            rev: v1.0.0
            hooks:
              - id: mise-system
    ", hook_repo});
    context.git().add(".");

    let ambient_data = context.work_dir().join("ambient-mise-data");
    let path = std::env::join_paths(
        std::iter::once(system_bin.to_path_buf()).chain(
            EnvVars
                .var_os(EnvVars::PATH)
                .as_ref()
                .into_iter()
                .flat_map(std::env::split_paths),
        ),
    )?;
    cmd_snapshot!(context, context.run()
        .env(EnvVars::PATH, path)
        .env(EnvVars::MISE_DATA_DIR, &ambient_data)
        .env("MISE_GLOBAL_CONFIG_FILE", "invalid ambient config")
        .env("__MISE_DIFF", "invalid inherited state"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    mise system..............................................................Passed
    - hook id: mise-system
    - duration: [TIME]

      zoxide 0.10.0

    ----- stderr -----
    ");
    assert!(
        !ambient_data.exists(),
        "Inherited MISE_DATA_DIR must not receive hook tools"
    );

    Ok(())
}
