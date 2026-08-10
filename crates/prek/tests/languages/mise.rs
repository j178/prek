use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::PathChild;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

#[cfg(unix)]
use crate::common::make_executable;
use crate::common::{TestContext, cmd_snapshot, git_cmd};

#[test]
fn managed_install_and_additional_dependencies() -> Result<()> {
    // This exercises release-backed installation in the three-platform language-test matrix.
    if !EnvVars.is_set(EnvVars::CI) {
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();

    let hook_repo = context.home_dir().child("mise-hook-repo");
    fs_err::create_dir_all(&hook_repo)?;
    fs_err::write(
        hook_repo.join(".pre-commit-hooks.yaml"),
        indoc::indoc! {r#"
            - id: mise-dependency
              name: mise dependency
              language: mise
              language_version: "=2026.7.18"
              entry: zoxide --version
              additional_dependencies: ["github:ajeetdsouza/zoxide@0.10.0"]
              always_run: true
              verbose: true
              pass_filenames: false
        "#},
    )?;
    // Only additional_dependencies define provisioning for a mise hook.
    fs_err::write(hook_repo.join("mise.toml"), "not valid = [")?;
    git_cmd(&hook_repo).arg("init").assert().success();
    git_cmd(&hook_repo).args(["add", "."]).assert().success();
    git_cmd(&hook_repo)
        .args(["commit", "-m", "Add mise hooks"])
        .assert()
        .success();
    let rev_output = git_cmd(&hook_repo).args(["rev-parse", "HEAD"]).output()?;
    let rev = String::from_utf8(rev_output.stdout)?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: '{}'
            rev: {}
            hooks:
              - id: mise-dependency
    ", hook_repo.display(), rev.trim()});
    // Make any fallback to the calling project's mise state fail loudly.
    fs_err::write(context.work_dir().join(".miserc.toml"), "not valid = [")?;
    context.git_add(".");

    let ambient_mise_dir = context.work_dir().join("ambient-mise-data");

    cmd_snapshot!(context.filters(), context.run()
        .env(EnvVars::PREK_INTERNAL__MISE_BINARY_NAME, "mise-never-exists")
        .env(EnvVars::MISE_DATA_DIR, &ambient_mise_dir)
        .env("__MISE_DIFF", "invalid inherited state"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    mise dependency..........................................................Passed
    - hook id: mise-dependency
    - duration: [TIME]

      zoxide 0.10.0

    ----- stderr -----
    "#);
    assert!(
        !ambient_mise_dir.exists(),
        "Inherited MISE_DATA_DIR must not receive hook tools"
    );

    Ok(())
}

#[cfg(unix)]
#[test]
fn system_mise_uses_hook_repository_and_prefers_activated_tools() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let hook_repo = context.home_dir().child("mise-system-hook-repo");
    fs_err::create_dir_all(hook_repo.join("tool"))?;
    fs_err::write(hook_repo.join("tool/marker"), "hook repository")?;
    let private_tool = hook_repo.join("tool/mise-test-tool");
    fs_err::write(
        &private_tool,
        indoc::indoc! {r#"
            #!/bin/sh
            set -eu

            test "${TEST_MISE_ACTIVATED-}" = "1"
            test "$PWD" -ef "$PREK_TEST_MISE_CALLER_CWD"
            test "$MISE_CEILING_PATHS" -ef "$PREK_TEST_MISE_CALLER_CWD"
            test ! -e tool/marker
            test "${MISE_NO_CONFIG-}" = "1"
            test "${MISE_DATA_DIR-}" != "$PREK_TEST_MISE_AMBIENT_DATA"
            test -z "${MISE_GLOBAL_CONFIG_FILE+x}"
            test -z "${__MISE_DIFF+x}"
            test "${PREK_TEST_MISE_HOOK_ENV-}" = "runtime-only"
            echo "private tool"
        "#},
    )?;
    make_executable(&private_tool)?;
    fs_err::write(
        hook_repo.join(".pre-commit-hooks.yaml"),
        indoc::indoc! {r#"
            - id: mise-system
              name: mise system
              language: mise
              language_version: system
              entry: mise-test-tool
              additional_dependencies: ["node@path:./tool"]
              always_run: true
              verbose: true
              pass_filenames: false
        "#},
    )?;
    git_cmd(&hook_repo).arg("init").assert().success();
    git_cmd(&hook_repo).args(["add", "."]).assert().success();
    git_cmd(&hook_repo)
        .args(["commit", "-m", "Add mise hook"])
        .assert()
        .success();
    let rev_output = git_cmd(&hook_repo).args(["rev-parse", "HEAD"]).output()?;
    let rev = String::from_utf8(rev_output.stdout)?;

    let bin_dir = context.home_dir().child("bin");
    fs_err::create_dir_all(&bin_dir)?;
    let fake_mise = bin_dir.join("mise-test");
    fs_err::write(
        &fake_mise,
        indoc::indoc! {r#"
            #!/bin/sh
            set -eu

            check_isolation() {
                test "${MISE_NO_CONFIG-}" = "1"
                test "${MISE_DATA_DIR-}" != "$PREK_TEST_MISE_AMBIENT_DATA"
                test -z "${MISE_GLOBAL_CONFIG_FILE+x}"
                test -z "${__MISE_DIFF+x}"
                test -z "${PREK_TEST_MISE_HOOK_ENV+x}"
            }

            case "${1-}" in
                --version)
                    test "$PWD" != "$PREK_TEST_MISE_CALLER_CWD"
                    test "$MISE_CEILING_PATHS" -ef "$PWD"
                    check_isolation
                    echo "2026.7.18 fake"
                    ;;
                --yes)
                    case "${2-}" in
                        install)
                            test "${3-}" = "--"
                            test "${4-}" = "node@path:./tool"
                            test -f tool/marker
                            test "$MISE_CEILING_PATHS" -ef "$PWD"
                            check_isolation
                            private_bin="$MISE_DATA_DIR/installs/mise-test-tool/latest"
                            mkdir -p "$private_bin"
                            cp tool/mise-test-tool "$private_bin/mise-test-tool"
                            chmod +x "$private_bin/mise-test-tool"
                            ;;
                        env)
                            test "${3-}" = "--json"
                            test "${4-}" = "--"
                            test "${5-}" = "node@path:./tool"
                            test -f tool/marker
                            test "$MISE_CEILING_PATHS" -ef "$PWD"
                            check_isolation
                            private_bin="$MISE_DATA_DIR/installs/mise-test-tool/latest"
                            test -x "$private_bin/mise-test-tool"
                            printf '{"PATH":"%s:%s","TEST_MISE_ACTIVATED":"1"}\n' "$private_bin" "$PATH"
                            ;;
                        *)
                            exit 2
                            ;;
                    esac
                    ;;
                *)
                    exit 2
                    ;;
            esac
        "#},
    )?;
    make_executable(&fake_mise)?;
    let system_tool = bin_dir.join("mise-test-tool");
    fs_err::write(
        &system_tool,
        indoc::indoc! {r#"
            #!/bin/sh
            echo "system tool"
            exit 1
        "#},
    )?;
    make_executable(&system_tool)?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: '{}'
            rev: {}
            hooks:
              - id: mise-system
                env:
                  PREK_TEST_MISE_HOOK_ENV: runtime-only
    ", hook_repo.display(), rev.trim()});
    context.git_add(".");

    let ambient_data = context.work_dir().join("ambient-mise-data");
    let path = std::env::join_paths(
        std::iter::once(bin_dir.to_path_buf()).chain(
            EnvVars
                .var_os(EnvVars::PATH)
                .as_ref()
                .into_iter()
                .flat_map(std::env::split_paths),
        ),
    )?;
    cmd_snapshot!(context.filters(), context.run()
        .env(EnvVars::PREK_INTERNAL__MISE_BINARY_NAME, "mise-test")
        .env(EnvVars::PATH, path)
        .env(EnvVars::MISE_DATA_DIR, &ambient_data)
        .env("MISE_GLOBAL_CONFIG_FILE", "invalid ambient config")
        .env("__MISE_DIFF", "invalid inherited state")
        .env("PREK_TEST_MISE_AMBIENT_DATA", &ambient_data)
        .env("PREK_TEST_MISE_CALLER_CWD", context.work_dir().to_path_buf())
        .env_remove("PREK_TEST_MISE_HOOK_ENV"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    mise system..............................................................Passed
    - hook id: mise-system
    - duration: [TIME]

      private tool

    ----- stderr -----
    ");

    Ok(())
}
