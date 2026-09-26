use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::prelude::*;
use predicates::prelude::predicate;
use prek_consts::env_vars::EnvVars;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PRE_COMMIT_CONFIG_YML, PREK_TOML};

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn run_does_not_rewrite_unchanged_config_tracking_file() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: noop
                name: Noop
                language: system
                entry: "true"
                always_run: true
    "#})
        .init_git();

    context.run().arg("--all-files").assert().success();

    let tracking_file = context.home_dir().child("config-tracking.json");
    tracking_file.assert(predicate::path::is_file());
    let original_content = fs_err::read_to_string(tracking_file.path())?;

    fs_err::OpenOptions::new()
        .write(true)
        .open(tracking_file.path())?
        .set_modified(SystemTime::UNIX_EPOCH)?;
    let original_modified = fs_err::metadata(tracking_file.path())?.modified()?;

    context.run().arg("--all-files").assert().success();

    assert_eq!(
        fs_err::read_to_string(tracking_file.path())?,
        original_content
    );
    assert_eq!(
        fs_err::metadata(tracking_file.path())?.modified()?,
        original_modified
    );

    Ok(())
}

#[test]
fn run_tracks_relative_config_as_absolute_path() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: noop
                name: Noop
                language: system
                entry: "true"
                always_run: true
    "#})
        .init_git();

    context
        .run()
        .arg("--config")
        .arg(PRE_COMMIT_CONFIG_YAML)
        .assert()
        .success();

    let tracking_file = context.home_dir().child("config-tracking.json");
    let tracked: Vec<PathBuf> =
        serde_json::from_str(&fs_err::read_to_string(tracking_file.path())?)?;

    assert_eq!(
        tracked,
        vec![context.child(PRE_COMMIT_CONFIG_YAML).path().to_path_buf()]
    );

    Ok(())
}

#[test]
fn invalid_config() {
    let context = TestEnv::new().with_config("invalid: config").init_git();

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: error: line 1 column 1: missing field `repos`
     --> <input>:1:1
      |
    1 | invalid: config
      | ^ missing field `repos`
    ");

    context.write_config(indoc::indoc! {r"
        files: 12
        repos: []
    "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: error: line 1 column 1: invalid type: integer `12`, expected a regex string or a mapping with `glob` set to a string or list of strings
     --> <input>:1:1
      |
    1 | files: 12
      | ^ invalid type: integer `12`, expected a regex string or a mapping with `glob` set to a string or list of strings
    2 | repos: []
      |
    ");

    context.write_config(indoc::indoc! {r#"
        files:
          glog: "*.rs"
        repos: []
    "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: error: line 2 column 3: unknown field `glog`, expected one of glob
     --> <input>:2:3
      |
    1 | files:
    2 |   glog: \"*.rs\"
      |   ^ unknown field `glog`, expected one of glob
    3 | repos: []
      |
    ");

    context.write_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: swift
                additional_dependencies: ["swift-format@5.0.0"]
                entry: echo Hello, world!
    "#});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `trailing-whitespace`
      caused by: Hook specified `additional_dependencies: swift-format@5.0.0` but the language `swift` does not support installing dependencies for now
    ");

    context.write_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: fail
                language_version: '6'
                entry: echo Hello, world!
    "});
    context.git().add(".");

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `trailing-whitespace`
      caused by: Hook specified `language_version: 6` but the language `fail` does not support toolchain installation for now
    ");
}

/// `.pre-commit-config.yaml` is not staged.
#[test]
fn config_not_staged() {
    let context = TestEnv::new()
        .with_file(PRE_COMMIT_CONFIG_YAML, "")
        .init_git();

    context.write_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -V
    "});

    cmd_snapshot!(context, context.run().arg("invalid-hook-id"), @r#"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Configuration file `.pre-commit-config.yaml` is not staged. Stage it with `git add` and try again
    "#);
}

/// `.pre-commit-config.yaml` outside the repository should not be checked.
#[test]
fn config_outside_repo() -> Result<()> {
    let context = TestEnv::new();

    // Initialize a git repository in ./work.
    let root = context.child("work");
    root.create_dir_all()?;
    context.git_at(&root).init();

    // Create a configuration file in . (outside the repository).
    let context = context.with_file(
        "c.yaml",
        indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: trailing-whitespace
                name: trailing-whitespace
                language: system
                entry: python3 -c 'print("Hello world")'
    "#},
    );

    cmd_snapshot!(context, context.run().current_dir(&root).arg("-c").arg("../c.yaml"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    trailing-whitespace..................................(no files to check)Skipped

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn global_path_options_expand_tilde() -> Result<()> {
    let context = TestEnv::new();
    let cd = context.home_dir().child("project");
    cd.create_dir_all()?;
    context.git_at(&cd).init();
    context
        .home_dir()
        .child("prek.toml")
        .write_str("repos = []\n")?;

    cmd_snapshot!(context, context
        .list()
        .arg("--config=~/prek.toml")
        .arg("--cd=~/project")
        .env("HOME", context.home_dir().path())
        .env("USERPROFILE", context.home_dir().path()), @r"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");

    Ok(())
}

/// Test `minimum_prek_version` option.
#[test]
fn minimum_prek_version() {
    let context = TestEnv::new()
        .with_filter(
            r"but version `\d+\.\d+\.\d+(?:-[0-9A-Za-z]+(?:\.[0-9A-Za-z]+)*)?` is installed",
            "but version `[CURRENT_VERSION]` is installed",
        )
        .with_config(indoc::indoc! {r"
        minimum_prek_version: 10.0.0
        repos:
          - repo: local
            hooks:
              - id: directory
                name: directory
                language: system
                entry: echo
                verbose: true
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: error: line 1 column 23: `prek` version `10.0.0` or newer is required, but version `[CURRENT_VERSION]` is installed. Upgrade `prek` and try again.
     --> <input>:1:23
      |
    1 | minimum_prek_version: 10.0.0
      |                       ^ `prek` version `10.0.0` or newer is required, but version `[CURRENT_VERSION]` is installed. Upgrade `prek` and try again.
    2 | repos:
    3 |   - repo: local
      |
    ");
}

/// Supports reading `pre-commit-config.yml` as well.
#[test]
fn alternate_config_file() {
    let context = TestEnv::new()
        .with_file(
            PRE_COMMIT_CONFIG_YML,
            indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python-hook
                name: local-python-hook
                language: python
                entry: python3 -c 'import sys; print("Hello, world!")'
    "#},
        )
        .init_git();

    cmd_snapshot!(context, context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    ");

    context.write_file(
        PRE_COMMIT_CONFIG_YAML,
        indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python-hook
                name: local-python-hook
                language: python
                entry: python3 -c 'import sys; print("Hello, world!")'
    "#},
    );
    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("--refresh").arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    warning: Multiple configuration files found (`.pre-commit-config.yaml`, `.pre-commit-config.yml`); using `[TEMP_DIR]/.pre-commit-config.yaml`
    ");

    context.write_file(
        PREK_TOML,
        indoc::indoc! {r#"
        [[repos]]
        repo = "local"
        hooks = [
          {
            id = "local-python-hook",
            name = "local-python-hook",
            language = "python",
            entry = "python3 -c 'import sys; print(\"Hello, world!\")'"
          }
        ]
    "#},
    );
    context.git().add(".");

    cmd_snapshot!(context, context.run().arg("--refresh").arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    warning: Multiple configuration files found (`prek.toml`, `.pre-commit-config.yaml`, `.pre-commit-config.yml`); using `[TEMP_DIR]/prek.toml`
    ");
}

/// Supports `prek.toml` as configuration file.
#[test]
fn prek_toml() {
    let context = TestEnv::new()
        .with_file(
            PREK_TOML,
            indoc::indoc! {r#"
        [[repos]]
        repo = "local"
        hooks = [
          {
            id = "local-python-hook",
            name = "local-python-hook",
            language = "python",
            entry = "python3 -c 'import sys; print(\"Hello, world!\")'"
          }
        ]
    "#},
        )
        .init_git();

    cmd_snapshot!(context, context.run().arg("-v"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    local-python-hook........................................................Passed
    - hook id: local-python-hook
    - duration: [TIME]

      Hello, world!

    ----- stderr -----
    ");
}

#[test]
fn prek_toml_resolves_priority_aliases() {
    let context = TestEnv::new()
        .with_file(
            PREK_TOML,
            indoc::indoc! {r#"
        [priorities]
        early = 0
        late = 10

        [[repos]]
        repo = "local"
        hooks = [
          {
            id = "late",
            name = "Late Hook",
            language = "system",
            entry = "python3 -c \"print('late')\"",
            always_run = true,
            priority = "late",
          },
          {
            id = "early",
            name = "Early Hook",
            language = "system",
            entry = "python3 -c \"print('early')\"",
            always_run = true,
            priority = "early",
          },
        ]
    "#},
        )
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Early Hook...............................................................Passed
    Late Hook................................................................Passed

    ----- stderr -----
    ");
}

#[test]
fn expands_tilde_in_prek_home() -> Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: ok
                name: ok
                entry: echo ok
                language: system
    "})
        .init_git();

    let fake_home = context.child("fake-home");
    fake_home.create_dir_all()?;

    cmd_snapshot!(context, context
        .run()
        .env("HOME", fake_home.path())
        .env("USERPROFILE", fake_home.path()) // For Windows
        .env(EnvVars::PREK_HOME, "~/prek-store"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ok.......................................................................Passed

    ----- stderr -----
    ");

    let store = fake_home.child("prek-store");
    store.child("README").assert(predicate::path::exists());
    store.child("repos").assert(predicate::path::is_dir());
    store.child("hooks").assert(predicate::path::is_dir());
    store.child("scratch").assert(predicate::path::is_dir());

    // Ensure we didn't create a literal `./~` directory under the project.
    context.child("~").assert(predicate::path::missing());

    Ok(())
}
