use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{PathChild, PathCreateDir};
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PREK_TOML};

use crate::common::{TestEnv, cmd_snapshot};

mod common;

#[test]
fn init_defaults_to_git_root() -> anyhow::Result<()> {
    let context = TestEnv::new().init_git();
    let child = context.child("child");
    child.create_dir_all()?;

    cmd_snapshot!(context, context.command().arg("init").current_dir(&child), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Created `[TEMP_DIR]/prek.toml`
    Installed Git hook at `../.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    assert!(context.child(PREK_TOML).is_file());
    assert!(!child.child(PREK_TOML).exists());

    Ok(())
}

#[test]
fn init_child_project_refreshes_parent_workspace() -> anyhow::Result<()> {
    let context = TestEnv::new().with_config("repos: []\n").init_git();
    context.list().assert().success();

    let child = context.child("child");
    child.create_dir_all()?;

    cmd_snapshot!(context, context.command().arg("init").arg("child"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Created `child/prek.toml`
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);
    assert!(child.child(PREK_TOML).is_file());

    cmd_snapshot!(context, context.list(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    child:trailing-whitespace
    child:end-of-file-fixer
    child:check-added-large-files

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn init_preserves_existing_config() {
    let config = "repos: []\n";
    let context = TestEnv::new().with_config(config).init_git();

    cmd_snapshot!(context, context.command().arg("init"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Found existing `.pre-commit-config.yaml`; skipping creation
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    assert_eq!(context.read(PRE_COMMIT_CONFIG_YAML), config);
    assert!(!context.child(PREK_TOML).exists());
}

#[test]
fn init_can_create_yaml_config() {
    let context = TestEnv::new().init_git();

    cmd_snapshot!(context, context.command().args(["init", "--format", "yaml"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Created `.pre-commit-config.yaml`
    Installed Git hook at `.git/hooks/pre-commit`

    ----- stderr -----
    "#);

    assert!(context.child(PRE_COMMIT_CONFIG_YAML).is_file());
    assert!(!context.child(PREK_TOML).exists());
}

#[test]
fn init_can_skip_hook_installation() {
    let context = TestEnv::new().init_git();

    cmd_snapshot!(context, context.command().args(["init", "--no-install"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Created `prek.toml`

    ----- stderr -----
    "#);

    assert!(context.child(PREK_TOML).is_file());
    assert!(!context.child(".git/hooks/pre-commit").exists());
}
