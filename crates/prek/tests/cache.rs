use assert_fs::assert::PathAssert;
use assert_fs::fixture::{ChildPath, PathChild, PathCreateDir};
use assert_fs::prelude::FileWriteStr;
use prek_consts::PRE_COMMIT_CONFIG_YAML;
use serde_json::json;
use std::path::Path;
use std::time::{Duration, SystemTime};

use crate::common::{TestEnv, cmd_snapshot};

mod common;

fn create_dirs<P>(root: &ChildPath, paths: impl IntoIterator<Item = P>) -> anyhow::Result<()>
where
    P: AsRef<Path>,
{
    for path in paths {
        root.child(path).create_dir_all()?;
    }
    Ok(())
}

fn write_json(path: &ChildPath, value: &impl serde::Serialize) -> anyhow::Result<()> {
    path.write_str(&serde_json::to_string_pretty(value)?)?;
    Ok(())
}

#[test]
fn cache_dir() {
    let context = TestEnv::new();
    let home = context.child("home");

    cmd_snapshot!(context, context.command().arg("cache").arg("dir").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [TEMP_DIR]/home

    ----- stderr -----
    ");
}

#[test]
fn cache_gc_verbose_shows_removed_entries() -> anyhow::Result<()> {
    let context = TestEnv::new().with_config("repos: []\n");
    let home = context.home_dir();

    // Seed store entries that will be removed.
    write_json(
        &home.child("repos/deadbeef/.prek-repo.json"),
        &json!({
            "repo": "https://github.com/pre-commit/pre-commit-hooks",
            "rev": "v1.0.0",
        }),
    )?;
    write_json(
        &home.child("hooks/hook-env-dead/.prek-hook.json"),
        &json!({
            "schema_version": 1,
            "language": "python",
            "language_version": "3.12.0",
            "repo": {
                "url": "https://example.com/repo",
                "rev": "v1.0.0",
            },
            "dependencies": [
                "dep1",
                "dep2",
                "dep3",
                "dep4",
                "dep5",
                "dep6",
                "dep7",
            ],
            "env_path": home.child("hooks/hook-env-dead").path(),
            "toolchain": "/usr/bin/python3",
            "extra": {},
        }),
    )?;

    home.child("cache/go").create_dir_all()?;

    // Have a tracked config that exists but references nothing (so everything above is unreferenced).
    let config_path = context.child(PRE_COMMIT_CONFIG_YAML);
    write_config_tracking_file(home, &[config_path.path()])?;

    cmd_snapshot!(context, context.command().args(["cache", "gc", "-v"]),@r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repo, 1 hook env, 1 cache entry ([SIZE])

    Removed 1 repo:
    - https://github.com/pre-commit/pre-commit-hooks@v1.0.0
      path: [HOME]/repos/deadbeef

    Removed 1 hook env:
    - python env
      path: [HOME]/hooks/hook-env-dead
      language: python (3.12.0)
      repo: https://example.com/repo@v1.0.0
      deps: dep1, dep2, dep3, dep4, dep5, dep6, … (+1 more)

    Removed 1 cache entry:
    - go
      path: [HOME]/cache/go

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn cache_clean() {
    let context = TestEnv::new()
        .with_filter(
            r"(?m)^Removed \d+ files? \([^)]+\)\n",
            "Removed [N] file(s) ([SIZE])\n",
        )
        .with_file("home/cache/data.bin", "hello")
        .with_file("home/cache/nested/data.bin", "world!");
    let home = context.child("home");

    cmd_snapshot!(context, context.command().arg("cache").arg("clean").env("PREK_HOME", &*home), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Removed [N] file(s) ([SIZE])
    ");

    home.assert(predicates::path::missing());

    // Test `prek clean` works for backward compatibility
    context.write_file("home/cache/one.txt", "abc");
    cmd_snapshot!(context, context.command().arg("clean").env("PREK_HOME", &*home), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    Removed [N] file(s) ([SIZE])
    ");

    home.assert(predicates::path::missing());
}

#[test]
fn cache_size_output_formats() {
    let context = TestEnv::new().with_filter(r"(?m)^\d+\n", "[BYTES]\n");

    cmd_snapshot!(context, context.command().args(["cache", "size", "--no-log-file", "--output-format", "auto"]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [BYTES]

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.command().args(["cache", "size", "--no-log-file", "--output-format", "human"]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [SIZE]

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.command().args(["cache", "size", "--no-log-file", "--output-format", "machine"]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [BYTES]

    ----- stderr -----
    ");
}

#[test]
fn cache_size_with_populated_cache() {
    let context = TestEnv::new()
        .with_filter(r"(?m)^\d+\n", "[BYTES]\n")
        .with_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v5.0.0
            hooks:
              - id: end-of-file-fixer
    "})
        .with_file("file.txt", "Hello, world!\n")
        .init_git();

    context.run();

    cmd_snapshot!(context, context.command().arg("cache").arg("size"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [BYTES]

    ----- stderr -----
    ");

    cmd_snapshot!(context, context.command().arg("cache").arg("size").arg("-H"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    [SIZE]

    ----- stderr -----
    ");
}

#[test]
fn cache_gc_removes_unreferenced_entries() -> anyhow::Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
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
    "#})
        .with_file("valid.yaml", "a: 1\n")
        .init_git();

    let home = context.home_dir();
    // Populate store + config tracking.
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    check yaml...............................................................Passed
    Python Hook..............................................................Passed

    ----- stderr -----
    ");

    // Add a few obviously-unused entries.
    create_dirs(
        home,
        [
            "repos/unused-repo",
            "hooks/unused-hook-env",
            "tools/node",
            "cache/go",
        ],
    )?;

    // Reduce hooks
    context.write_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/pre-commit/pre-commit-hooks
            rev: v6.0.0
            hooks:
              - id: check-yaml
        "});

    cmd_snapshot!(context, context.command().arg("cache").arg("gc"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repo, 2 hook envs, 1 tool, 1 cache entry ([SIZE])

    ----- stderr -----
    "#);

    home.child("repos/unused-repo")
        .assert(predicates::path::missing());
    home.child("hooks/unused-hook-env")
        .assert(predicates::path::missing());
    home.child("tools/node").assert(predicates::path::missing());
    home.child("cache/go").assert(predicates::path::missing());

    Ok(())
}

#[test]
fn cache_gc_keeps_relative_remote_repo() -> anyhow::Result<()> {
    let context = TestEnv::new()
        .with_file(
            "hook-repo/.pre-commit-hooks.yaml",
            indoc::indoc! {r"
        - id: test-hook
          name: Test Hook
          entry: echo test
          language: system
          always_run: true
    "},
        )
        .init_git();
    let hook_repo = context.child("hook-repo");
    let git = context.git_at(&hook_repo);
    let revision = git
        .init()
        .add(".")
        .commit("Initial commit")
        .rev_parse("HEAD")?;

    let context = context.with_file(
        "subproject/.pre-commit-config.yaml",
        indoc::formatdoc! {r"
            repos:
              - repo: ../hook-repo
                rev: {revision}
                hooks:
                  - id: test-hook
        "},
    );
    context.git().add(".");

    cmd_snapshot!(context, context.run()
        .arg("--config")
        .arg("subproject/.pre-commit-config.yaml"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Test Hook................................................................Passed

    ----- stderr -----
    ");

    let repos_dir = context.home_dir().child("repos");
    let cached_repo = fs_err::read_dir(repos_dir.path())?
        .next()
        .transpose()?
        .expect("expected the relative remote repo to be cached")
        .path();
    repos_dir.child("unused-repo").create_dir_all()?;

    cmd_snapshot!(context, context.command().args(["cache", "gc"]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repo ([SIZE])

    ----- stderr -----
    ");

    assert!(cached_repo.is_dir(), "cache GC removed the configured repo");
    repos_dir
        .child("unused-repo")
        .assert(predicates::path::missing());

    Ok(())
}

#[test]
fn cache_gc_prunes_unused_tool_versions() -> anyhow::Result<()> {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python
                name: Local Python Hook
                entry: "python -c \"print(1)\""
                language: python
              - id: local-pygrep
                name: Local Pygrep Hook
                entry: "python -c \"print(1)\""
                language: pygrep
              - id: local-node
                name: Local Node Hook
                entry: "node -e \"console.log(1)\""
                language: node
              - id: local-go
                name: Local Go Hook
                entry: "go version"
                language: golang
              - id: local-ruby
                name: Local Ruby Hook
                entry: "ruby -e 'puts 1'"
                language: ruby
              - id: local-rust
                name: Local Rust Hook
                entry: "rustc --version"
                language: rust
    "#});

    let home = context.home_dir();

    // Track the config so GC has something to mark from.
    let config_path = context.child(PRE_COMMIT_CONFIG_YAML);
    write_config_tracking_file(home, &[config_path.path()])?;

    // Seed "used" hook env markers so GC can read `.prek-hook.json` and retain the
    // corresponding tool versions per language.
    create_dirs(
        home,
        [
            "hooks/python-keep",
            "hooks/node-keep",
            "hooks/go-keep",
            "hooks/ruby-remove",
            "hooks/rust-remove",
            "tools/python/3.12.0",
            "tools/python/3.11.0",
            "tools/node/22.0.0",
            "tools/node/21.0.0",
            "tools/go/1.24.0",
            "tools/go/1.23.0",
        ],
    )?;

    let env_py = home.child("hooks/python-keep");
    let env_node = home.child("hooks/node-keep");
    let env_go = home.child("hooks/go-keep");
    let py_keep = home.child("tools/python/3.12.0");
    let node_keep = home.child("tools/node/22.0.0");
    let go_keep = home.child("tools/go/1.24.0");

    // Match logic for local hooks: empty deps + language request is `Any` by default.
    let marker_py = json!({
        "schema_version": 1,
        "language": "python",
        "language_version": "3.12.0",
        "dependencies": [],
        "env_path": env_py.path(),
        "toolchain": py_keep.child("bin/python").path(),
        "extra": {},
    });
    write_json(&env_py.child(".prek-hook.json"), &marker_py)?;

    let marker_node = json!({
        "schema_version": 1,
        "language": "node",
        "language_version": "22.0.0",
        "dependencies": [],
        "env_path": env_node.path(),
        "toolchain": node_keep.child("bin/node").path(),
        "extra": {},
    });
    write_json(&env_node.child(".prek-hook.json"), &marker_node)?;

    let marker_go = json!({
        "schema_version": 1,
        "language": "golang",
        "language_version": "1.24.0",
        "dependencies": [],
        "env_path": env_go.path(),
        "toolchain": go_keep.child("bin/go").path(),
        "extra": {},
    });
    write_json(&env_go.child(".prek-hook.json"), &marker_go)?;

    cmd_snapshot!(context, context.command().args(["cache", "gc", "--dry-run", "-v"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Would remove 2 hook envs, 3 tools ([SIZE])

    Would remove 2 hook envs:
    - ruby-remove
      path: [HOME]/hooks/ruby-remove
    - rust-remove
      path: [HOME]/hooks/rust-remove

    Would remove 3 tools:
    - go/1.23.0
      path: [HOME]/tools/go/1.23.0
    - node/21.0.0
      path: [HOME]/tools/node/21.0.0
    - python/3.11.0
      path: [HOME]/tools/python/3.11.0

    ----- stderr -----
    "#);

    cmd_snapshot!(context, context.command().args(["cache", "gc", "-v"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 2 hook envs, 3 tools ([SIZE])

    Removed 2 hook envs:
    - ruby-remove
      path: [HOME]/hooks/ruby-remove
    - rust-remove
      path: [HOME]/hooks/rust-remove

    Removed 3 tools:
    - go/1.23.0
      path: [HOME]/tools/go/1.23.0
    - node/21.0.0
      path: [HOME]/tools/node/21.0.0
    - python/3.11.0
      path: [HOME]/tools/python/3.11.0

    ----- stderr -----
    "#);

    Ok(())
}

#[test]
fn cache_gc_prunes_tool_versions_without_positive_identification() -> anyhow::Result<()> {
    let context = TestEnv::new().with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python
                name: Local Python Hook
                entry: "python -c \"print(1)\""
                language: python
    "#});

    let home = context.home_dir();

    // Track the config so GC has something to mark from.
    let config_path = context.child(PRE_COMMIT_CONFIG_YAML);
    write_config_tracking_file(home, &[config_path.path()])?;

    // Seed a matching installed hook env marker, but use a toolchain path that is *not* inside
    // PREK_HOME/tools. This means we cannot positively identify a used tool version, so all
    // tool versions under the bucket are unused and should be pruned.
    create_dirs(
        home,
        [
            "hooks/python-keep",
            "tools/python/3.12.0",
            "tools/python/3.11.0",
            "repos/.temp",
            "tools/.temp",
        ],
    )?;
    let env_py = home.child("hooks/python-keep");
    let py_312 = home.child("tools/python/3.12.0");
    let py_311 = home.child("tools/python/3.11.0");
    let marker_py = json!({
        "schema_version": 1,
        "language": "python",
        "language_version": "3.12.0",
        "dependencies": [],
        "env_path": env_py.path(),
        "toolchain": "/usr/bin/python3",
        "extra": {},
    });
    write_json(&env_py.child(".prek-hook.json"), &marker_py)?;

    cmd_snapshot!(context,
        context.command().args(["cache", "gc", "--dry-run", "-v"]),
        @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Would remove 2 tools ([SIZE])

    Would remove 2 tools:
    - python/3.11.0
      path: [HOME]/tools/python/3.11.0
    - python/3.12.0
      path: [HOME]/tools/python/3.12.0

    ----- stderr -----
    "
    );

    cmd_snapshot!(context, context.command().args(["cache", "gc"]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 2 tools ([SIZE])

    ----- stderr -----
    ");

    py_312.assert(predicates::path::missing());
    py_311.assert(predicates::path::missing());
    home.child("tools/python")
        .assert(predicates::path::is_dir());

    Ok(())
}

#[test]
fn cache_gc_keeps_local_hook_env() -> anyhow::Result<()> {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python
                name: Local Python Hook
                entry: python -c "print('hello')"
                language: python
    "#})
        .with_file("file.txt", "Hello\n")
        .init_git();

    // Install + run the local hook so it creates a hook env under PREK_HOME/hooks.
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Local Python Hook........................................................Passed

    ----- stderr -----
    ");

    let home = context.home_dir();
    let hooks_dir = home.child("hooks");

    let mut local_envs = Vec::new();
    for entry in fs_err::read_dir(hooks_dir.path())? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("python-") {
            local_envs.push(name);
        }
    }

    assert!(
        !local_envs.is_empty(),
        "expected at least one local hook env"
    );

    // Add an obviously-unused entry to ensure GC does work.
    home.child("hooks/unused-hook-env").create_dir_all()?;

    cmd_snapshot!(context, context.command().args(["cache", "gc"]), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 hook env ([SIZE])

    ----- stderr -----
    "#);

    // The local hook env(s) should remain.
    for env in local_envs {
        home.child(format!("hooks/{env}"))
            .assert(predicates::path::is_dir());
    }
    // Unused should be swept.
    home.child("hooks/unused-hook-env")
        .assert(predicates::path::missing());

    Ok(())
}

#[test]
fn cache_gc_removes_stale_patch_files() -> anyhow::Result<()> {
    let context = TestEnv::new().with_config("repos: []\n");

    let home = context.home_dir();
    let config_path = context.child(PRE_COMMIT_CONFIG_YAML);
    write_config_tracking_file(home, &[config_path.path()])?;

    let old_patch = home.child("patches/old.patch");
    let recent_patch = home.child("patches/recent.patch");

    write_patch_file(
        &old_patch,
        "old patch\n",
        SystemTime::now() - Duration::from_hours(60 * 24),
    )?;
    write_patch_file(
        &recent_patch,
        "recent patch\n",
        SystemTime::now() - Duration::from_hours(24),
    )?;

    cmd_snapshot!(context, context.command().args(["cache", "gc", "-v", "--dry-run"]), @"
    success: true
    exit_code: 0
    ----- stdout -----
    Would remove 1 patch file ([SIZE])

    Would remove 1 patch file:
    - old.patch
      path: [HOME]/patches/old.patch

    ----- stderr -----
    ");
    old_patch.assert(predicates::path::is_file());
    recent_patch.assert(predicates::path::is_file());

    cmd_snapshot!(context, context.command().args(["cache", "gc", "-v"]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 patch file ([SIZE])

    Removed 1 patch file:
    - old.patch
      path: [HOME]/patches/old.patch

    ----- stderr -----
    ");

    old_patch.assert(predicates::path::missing());
    recent_patch.assert(predicates::path::is_file());

    Ok(())
}

fn write_config_tracking_file(
    home: &ChildPath,
    configs: &[&std::path::Path],
) -> anyhow::Result<()> {
    let configs: Vec<String> = configs
        .iter()
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    write_json(&home.child("config-tracking.json"), &configs)
}

fn write_patch_file(path: &ChildPath, content: &str, modified: SystemTime) -> anyhow::Result<()> {
    path.write_str(content)?;
    fs_err::OpenOptions::new()
        .write(true)
        .open(path.path())?
        .set_modified(modified)?;
    Ok(())
}

#[test]
fn cache_gc_drops_missing_tracked_config() -> anyhow::Result<()> {
    let context = TestEnv::new().with_config("repos: []\n").init_git();

    let home = context.home_dir();
    let config_path = context.child(PRE_COMMIT_CONFIG_YAML);
    write_config_tracking_file(home, &[config_path.path()])?;

    // Simulate config being deleted between runs.
    fs_err::remove_file(config_path.path())?;

    // Add a few obviously-unused entries to ensure GC sweeps.
    create_dirs(
        home,
        [
            "repos/unused-repo",
            "hooks/unused-hook-env",
            "tools/node",
            "cache/go",
            "scratch/some-temp",
            "patches/some-patch",
        ],
    )?;

    cmd_snapshot!(context, context.command().arg("cache").arg("gc"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repo, 1 hook env, 1 tool, 1 cache entry ([SIZE])

    ----- stderr -----
    "#);

    // Tracking file should be updated to drop the missing config.
    let content = fs_err::read_to_string(home.child("config-tracking.json").path())?;
    let tracked: Vec<String> = serde_json::from_str(&content)?;
    assert!(tracked.is_empty());

    // Scratch is always cleared. Patch directories remain unless they contain stale patch files.
    home.child("scratch").assert(predicates::path::missing());
    home.child("patches").assert(predicates::path::is_dir());

    Ok(())
}

#[test]
fn cache_gc_keeps_tracked_config_on_parse_error() -> anyhow::Result<()> {
    // Keep the tracked config intentionally invalid while exercising GC.
    let context = TestEnv::new()
        .with_file(PRE_COMMIT_CONFIG_YAML, "repos: [\n")
        .init_git();

    let home = context.home_dir();
    let config_path = context.child(PRE_COMMIT_CONFIG_YAML);
    write_config_tracking_file(home, &[config_path.path()])?;

    // Add a few obviously-unused entries to ensure GC sweeps even when config is unparsable.
    create_dirs(
        home,
        [
            "repos/unused-repo",
            "hooks/unused-hook-env",
            "tools/node",
            "cache/go",
        ],
    )?;

    cmd_snapshot!(context, context.command().arg("cache").arg("gc"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repo, 1 hook env, 1 tool, 1 cache entry ([SIZE])

    ----- stderr -----
    "#);

    // Parse errors should not drop the config from tracking.
    let content = fs_err::read_to_string(home.child("config-tracking.json").path())?;
    let tracked: Vec<String> = serde_json::from_str(&content)?;
    assert_eq!(tracked.len(), 1);

    Ok(())
}

#[test]
fn cache_gc_dry_run_does_not_remove_entries() -> anyhow::Result<()> {
    let context = TestEnv::new().with_config("repos: []\n").init_git();

    let home = context.home_dir();
    // Seed tracking with a missing config to force sweeping everything.
    let missing_config_path = context.child("missing-config.yaml");
    write_config_tracking_file(home, &[missing_config_path.path()])?;

    create_dirs(
        home,
        [
            "repos/unused-repo",
            "hooks/unused-hook-env",
            "tools/node",
            "cache/go",
            "scratch/some-temp",
        ],
    )?;

    cmd_snapshot!(context, context.command().arg("cache").arg("gc").arg("--dry-run"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Would remove 1 repo, 1 hook env, 1 tool, 1 cache entry ([SIZE])

    ----- stderr -----
    "#);

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
