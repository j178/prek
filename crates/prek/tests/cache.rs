use assert_fs::assert::PathAssert;
use assert_fs::fixture::{ChildPath, PathChild, PathCreateDir};
use assert_fs::prelude::FileWriteStr;
use prek_consts::CONFIG_FILE;
use serde_json::json;

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
    home.child("repos/deadbeef/.prek-repo.json")
        .write_str(
            &serde_json::to_string_pretty(&json!({
                "repo": "https://github.com/pre-commit/pre-commit-hooks",
                "rev": "v1.0.0",
            }))
            .expect("serialize repo marker"),
        )
        .expect("write repo marker");
    home.child("hooks/hook-env-dead")
        .create_dir_all()
        .expect("create hook env dir");
    home.child("hooks/hook-env-dead/.prek-hook.json")
        .write_str(
            &serde_json::to_string_pretty(&json!({
                "language": "python",
                "language_version": "3.12.0",
                "dependencies": [
                    "https://example.com/repo@v1.0.0",
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
            }))
            .expect("serialize hook marker"),
        )
        .expect("write hook marker");

    home.child("cache/go")
        .create_dir_all()
        .expect("create cache dir");

    // Have a tracked config that exists but references nothing (so everything above is unreferenced).
    let config_path = context.work_dir().child(CONFIG_FILE);
    write_config_tracking_file(home, &[config_path.path()]).expect("write tracking file");

    cmd_snapshot!(context.filters(), context.command().args(["cache", "gc", "-v"]),@r"
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
    Removed 1 repo, 2 hook envs, 1 tool, 1 cache entry ([SIZE])

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

#[test]
fn cache_gc_prunes_unused_tool_versions() -> anyhow::Result<()> {
    let context = TestContext::new();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    let config_path = context.work_dir().child(CONFIG_FILE);
    write_config_tracking_file(home, &[config_path.path()])?;

    // Seed "used" hook env markers so GC can read `.prek-hook.json` and retain the
    // corresponding tool versions per language.
    let env_py = home.child("hooks/python-keep");
    let env_node = home.child("hooks/node-keep");
    let env_go = home.child("hooks/go-keep");
    let env_ruby = home.child("hooks/ruby-remove");
    let env_rust = home.child("hooks/rust-remove");
    env_py.create_dir_all()?;
    env_node.create_dir_all()?;
    env_go.create_dir_all()?;
    env_ruby.create_dir_all()?;
    env_rust.create_dir_all()?;

    let py_keep = home.child("tools/python/3.12.0");
    let py_remove = home.child("tools/python/3.11.0");
    py_keep.create_dir_all()?;
    py_remove.create_dir_all()?;

    let node_keep = home.child("tools/node/22.0.0");
    let node_remove = home.child("tools/node/21.0.0");
    node_keep.create_dir_all()?;
    node_remove.create_dir_all()?;

    let go_keep = home.child("tools/go/1.24.0");
    let go_remove = home.child("tools/go/1.23.0");
    go_keep.create_dir_all()?;
    go_remove.create_dir_all()?;

    // Match logic for local hooks: empty deps + language request is `Any` by default.
    let marker_py = json!({
        "language": "python",
        "language_version": "3.12.0",
        "dependencies": [],
        "env_path": env_py.path(),
        "toolchain": py_keep.child("bin/python").path(),
        "extra": {},
    });
    env_py
        .child(".prek-hook.json")
        .write_str(&serde_json::to_string_pretty(&marker_py)?)?;

    let marker_node = json!({
        "language": "node",
        "language_version": "22.0.0",
        "dependencies": [],
        "env_path": env_node.path(),
        "toolchain": node_keep.child("bin/node").path(),
        "extra": {},
    });
    env_node
        .child(".prek-hook.json")
        .write_str(&serde_json::to_string_pretty(&marker_node)?)?;

    let marker_go = json!({
        "language": "golang",
        "language_version": "1.24.0",
        "dependencies": [],
        "env_path": env_go.path(),
        "toolchain": go_keep.child("bin/go").path(),
        "extra": {},
    });
    env_go
        .child(".prek-hook.json")
        .write_str(&serde_json::to_string_pretty(&marker_go)?)?;

    cmd_snapshot!(context.filters(), context.command().args(["cache", "gc", "--dry-run", "-v"]), @r"
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
    ");

    cmd_snapshot!(context.filters(), context.command().args(["cache", "gc", "-v"]), @r"
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
    ");

    Ok(())
}

#[test]
fn cache_gc_prunes_tool_versions_without_positive_identification() -> anyhow::Result<()> {
    let context = TestContext::new();

    context.write_pre_commit_config(indoc::indoc! {r#"
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
    let config_path = context.work_dir().child(CONFIG_FILE);
    write_config_tracking_file(home, &[config_path.path()])?;

    // Seed a matching installed hook env marker, but use a toolchain path that is *not* inside
    // PREK_HOME/tools. This means we cannot positively identify a used tool version, so all
    // tool versions under the bucket are unused and should be pruned.
    let env_py = home.child("hooks/python-keep");
    env_py.create_dir_all()?;
    let marker_py = json!({
        "language": "python",
        "language_version": "3.12.0",
        "dependencies": [],
        "env_path": env_py.path(),
        "toolchain": "/usr/bin/python3",
        "extra": {},
    });
    env_py
        .child(".prek-hook.json")
        .write_str(&serde_json::to_string_pretty(&marker_py)?)?;

    // Seed tool versions that should be removed.
    let py_312 = home.child("tools/python/3.12.0");
    let py_311 = home.child("tools/python/3.11.0");
    py_312.create_dir_all()?;
    py_311.create_dir_all()?;

    // Add a temp dir to ensure it is not removed.
    home.child("repos/.temp").create_dir_all()?;
    home.child("tools/.temp").create_dir_all()?;

    cmd_snapshot!(
        context.filters(),
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

    cmd_snapshot!(context.filters(), context.command().args(["cache", "gc"]), @r"
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
    let context = TestContext::new();
    context.init_project();

    let cwd = context.work_dir();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-python
                name: Local Python Hook
                entry: python -c "print('hello')"
                language: python
    "#});

    cwd.child("file.txt").write_str("Hello\n")?;
    context.git_add(".");

    // Install + run the local hook so it creates a hook env under PREK_HOME/hooks.
    cmd_snapshot!(context.filters(), context.run(), @r"
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

        let name = entry.file_name().to_string_lossy().to_string();
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

    cmd_snapshot!(context.filters(), context.command().args(["cache", "gc"]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 hook env ([SIZE])

    ----- stderr -----
    ");

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

fn write_workspace_cache_file(
    home: &ChildPath,
    workspace_root: &std::path::Path,
) -> anyhow::Result<()> {
    use std::hash::{Hash as _, Hasher as _};
    use std::time::SystemTime;

    let config_path = workspace_root.join(CONFIG_FILE);
    let metadata = fs_err::metadata(&config_path)?;
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let size = metadata.len();

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    workspace_root.hash(&mut hasher);
    let digest = hex::encode(hasher.finish().to_le_bytes());

    let cache_path = home.child("cache/prek/workspace").child(digest);
    let parent = cache_path.parent().expect("cache path has parent");
    fs_err::create_dir_all(parent)?;

    let content = json!({
        "version": 1u32,
        "workspace_root": workspace_root,
        "created_at": serde_json::to_value(SystemTime::now())?,
        "config_files": [
            {
                "path": config_path,
                "modified": serde_json::to_value(modified)?,
                "size": size,
            }
        ],
    });

    cache_path.write_str(&serde_json::to_string_pretty(&content)?)?;
    Ok(())
}

#[test]
fn cache_gc_bootstraps_tracking_from_workspace_cache() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config("repos: []\n");
    context.git_add(".");

    let home = context.home_dir();
    write_workspace_cache_file(home, context.work_dir().path())?;

    // Seed store entries that should be swept, even if `config-tracking.json` is missing.
    home.child("repos/deadbeef").create_dir_all()?;
    home.child("hooks/hook-env-dead").create_dir_all()?;

    cmd_snapshot!(context.filters(), context.command().arg("cache").arg("gc"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Removed 1 repo, 1 hook env ([SIZE])

    ----- stderr -----
    ");

    home.child("repos/deadbeef")
        .assert(predicates::path::missing());
    home.child("hooks/hook-env-dead")
        .assert(predicates::path::missing());

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
    Removed 1 repo, 1 hook env, 1 tool, 1 cache entry ([SIZE])

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
    Removed 1 repo, 1 hook env, 1 tool, 1 cache entry ([SIZE])

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
    Would remove 1 repo, 1 hook env, 1 tool, 1 cache entry ([SIZE])

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
