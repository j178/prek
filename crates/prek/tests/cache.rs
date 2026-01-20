use assert_fs::assert::PathAssert;
use assert_fs::fixture::{ChildPath, PathChild, PathCreateDir};
use assert_fs::prelude::FileWriteStr;
use prek_consts::CONFIG_FILE;

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn cache_dir() {
    let context = TestContext::new();
    let home = context.work_dir().child("home");

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("dir").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [TEMP_DIR]/home

    ----- stderr -----
    ");
}

#[test]
fn cache_gc_verbose_shows_removed_entries() {
    let context = TestContext::new();

    context.write_pre_commit_config("repos: []\n");
    let home = context.home_dir();

    // Seed store entries that will be removed.
    home.child("repos/deadbeef")
        .create_dir_all()
        .expect("create repo dir");
    home.child("hooks/hook-env-dead")
        .create_dir_all()
        .expect("create hook env dir");

    // Have a tracked config that exists but references nothing (so everything above is unreferenced).
    let config_path = context.work_dir().child(CONFIG_FILE);
    write_config_tracking_file(home, &[config_path.path()]).expect("write tracking file");

    cmd_snapshot!(context.filters(), context
        .command()
        .args(["cache", "gc", "-v"]),
        @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repos, 1 hook envs

    Removed 1 repos:
    - deadbeef

    Removed 1 hook envs:
    - hook-env-dead

    ----- stderr -----
    ");
}

#[test]
fn cache_clean() -> anyhow::Result<()> {
    let context = TestContext::new();

    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("clean").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Cleaned `[TEMP_DIR]/home`

    ----- stderr -----
    ");

    home.assert(predicates::path::missing());

    // Test `prek clean` works for backward compatibility
    home.create_dir_all()?;
    cmd_snapshot!(context.filters(), context.command().arg("clean").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Cleaned `[TEMP_DIR]/home`

    ----- stderr -----
    ");

    home.assert(predicates::path::missing());

    Ok(())
}

#[test]
fn cache_size() -> anyhow::Result<()> {
    let context = TestContext::new().with_filtered_cache_size();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: end-of-file-fixer
    "});

    cwd.child("file.txt").write_str("Hello, world!\n")?;
    context.git_add(".");

    context.run();

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("size"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [SIZE]

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("size").arg("-H"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [SIZE]

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn cache_gc_removes_unreferenced_entries() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v6.0.0
            hooks:
              - id: check-yaml
          - repo: local
            hooks:
              - id: python-hook
                name: Python Hook
                entry: python -c "print('Hello from Python')"
                language: python
    "#});

    cwd.child("valid.yaml").write_str("a: 1\n")?;
    context.git_add(".");

    let home = context.home_dir();
    // Populate store + config tracking.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check yaml...............................................................Passed
    Python Hook..............................................................Passed

    ----- stderr -----
    ");

    // Add a few obviously-unused entries.
    home.child("repos/unused-repo").create_dir_all()?;
    home.child("hooks/unused-hook-env").create_dir_all()?;
    home.child("tools/node").create_dir_all()?;
    home.child("cache/go").create_dir_all()?;

    // Reduce hooks
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v6.0.0
            hooks:
              - id: check-yaml
    "});

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("gc"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repos, 2 hook envs, 1 tools, 1 cache entries

    ----- stderr -----
    ");

    home.child("repos/unused-repo")
        .assert(predicates::path::missing());
    home.child("hooks/unused-hook-env")
        .assert(predicates::path::missing());
    home.child("tools/node").assert(predicates::path::missing());
    home.child("cache/go").assert(predicates::path::missing());

    Ok(())
}

fn write_config_tracking_file(
    home: &ChildPath,
    configs: &[&std::path::Path],
) -> anyhow::Result<()> {
    let configs: Vec<String> = configs
        .iter()
        .map(|p| p.to_string_lossy().to_string())
        .collect();
    let content = serde_json::to_string_pretty(&configs)?;
    home.child("config-tracking.json").write_str(&content)?;
    Ok(())
}

#[test]
fn cache_gc_drops_missing_tracked_config() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config("repos: []\n");
    context.git_add(".");

    let home = context.home_dir();
    let config_path = cwd.child(CONFIG_FILE);
    write_config_tracking_file(home, &[config_path.path()])?;

    // Simulate config being deleted between runs.
    fs_err::remove_file(config_path.path())?;

    // Add a few obviously-unused entries to ensure GC sweeps.
    home.child("repos/unused-repo").create_dir_all()?;
    home.child("hooks/unused-hook-env").create_dir_all()?;
    home.child("tools/node").create_dir_all()?;
    home.child("cache/go").create_dir_all()?;
    home.child("scratch/some-temp").create_dir_all()?;
    home.child("patches/some-patch").create_dir_all()?;

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("gc"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repos, 1 hook envs, 1 tools, 1 cache entries

    ----- stderr -----
    ");

    // Tracking file should be updated to drop the missing config.
    let content = fs_err::read_to_string(home.child("config-tracking.json").path())?;
    let tracked: Vec<String> = serde_json::from_str(&content)?;
    assert!(tracked.is_empty());

    // Scratch and patches are always cleared when GC runs.
    home.child("scratch").assert(predicates::path::missing());
    home.child("patches").assert(predicates::path::is_dir());

    Ok(())
}

#[test]
fn cache_gc_keeps_tracked_config_on_parse_error() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    // Intentionally invalid YAML.
    cwd.child(CONFIG_FILE).write_str("repos: [\n")?;
    context.git_add(".");

    let home = context.home_dir();
    let config_path = cwd.child(CONFIG_FILE);
    write_config_tracking_file(home, &[config_path.path()])?;

    // Add a few obviously-unused entries to ensure GC sweeps even when config is unparsable.
    home.child("repos/unused-repo").create_dir_all()?;
    home.child("hooks/unused-hook-env").create_dir_all()?;
    home.child("tools/node").create_dir_all()?;
    home.child("cache/go").create_dir_all()?;

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("gc"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repos, 1 hook envs, 1 tools, 1 cache entries

    ----- stderr -----
    ");

    // Parse errors should not drop the config from tracking.
    let content = fs_err::read_to_string(home.child("config-tracking.json").path())?;
    let tracked: Vec<String> = serde_json::from_str(&content)?;
    assert_eq!(tracked.len(), 1);

    Ok(())
}

#[test]
fn cache_gc_dry_run_does_not_remove_entries() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config("repos: []\n");
    context.git_add(".");

    let home = context.home_dir();
    // Seed tracking with a missing config to force sweeping everything.
    let missing_config_path = cwd.child("missing-config.yaml");
    write_config_tracking_file(home, &[missing_config_path.path()])?;

    home.child("repos/unused-repo").create_dir_all()?;
    home.child("hooks/unused-hook-env").create_dir_all()?;
    home.child("tools/node").create_dir_all()?;
    home.child("cache/go").create_dir_all()?;
    home.child("scratch/some-temp").create_dir_all()?;

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("gc").arg("--dry-run"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Would remove 1 repos, 1 hook envs, 1 tools, 1 cache entries

    ----- stderr -----
    ");

    // Nothing should be removed in dry-run mode.
    home.child("repos/unused-repo")
        .assert(predicates::path::is_dir());
    home.child("hooks/unused-hook-env")
        .assert(predicates::path::is_dir());
    home.child("tools/node").assert(predicates::path::is_dir());
    home.child("cache/go").assert(predicates::path::is_dir());
    home.child("scratch/some-temp")
        .assert(predicates::path::is_dir());

    Ok(())
}
