use std::env::consts::EXE_EXTENSION;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use crate::common::{TestContext, cmd_snapshot, git_cmd, make_executable};

#[test]
fn reuses_managed_mise() {
    if !EnvVars.is_set(EnvVars::CI) {
        return;
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
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
    "#});
    context.git_add(".");

    let mut filters = context.filters();
    filters.push((
        r"2026\.7\.18 [^\r\n]+ \(\d{4}-\d{2}-\d{2}\)",
        "2026.7.18 [PLATFORM] ([DATE])",
    ));

    cmd_snapshot!(filters.clone(), context.run()
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
    context.write_pre_commit_config(indoc::indoc! {r"
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
    context.git_add(".");

    cmd_snapshot!(filters, context.run()
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
    if !EnvVars.is_set(EnvVars::CI) {
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();

    let hook_repo = context.home_dir().child("mise-system-hook-repo");
    fs_err::create_dir_all(&hook_repo)?;
    fs_err::write(
        hook_repo.join(".pre-commit-hooks.yaml"),
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
    )?;
    // Provisioning must not read configuration from the hook repository.
    fs_err::write(hook_repo.join("mise.toml"), "not valid = [")?;
    git_cmd(&hook_repo).arg("init").assert().success();
    git_cmd(&hook_repo).args(["add", "."]).assert().success();
    git_cmd(&hook_repo)
        .args(["commit", "-m", "Add mise hook"])
        .assert()
        .success();
    let rev_output = git_cmd(&hook_repo).args(["rev-parse", "HEAD"]).output()?;
    let rev = String::from_utf8(rev_output.stdout)?;

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

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: '{}'
            rev: {}
            hooks:
              - id: mise-system
    ", hook_repo.display(), rev.trim()});
    // Early miserc discovery must not read configuration from the calling project.
    fs_err::write(context.work_dir().join(".miserc.toml"), "not valid = [")?;
    context.git_add(".");

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
    cmd_snapshot!(context.filters(), context.run()
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
