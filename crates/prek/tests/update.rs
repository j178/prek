use std::process::Command;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::ChildPath;
use assert_fs::prelude::*;
use insta::assert_snapshot;
use prek_consts::{PRE_COMMIT_CONFIG_YAML, PREK_TOML};

use crate::common::{TestEnv, TestGit, cmd_snapshot, snapshot};

mod common;

const BASE_TIMESTAMP: u64 = 1_000_000_000;
const INCREMENTING_STEP_SECS: u64 = 100;
const FIXED_STEP_SECS: u64 = 0;

fn git_command_at(git: &TestGit<'_>, timestamp: u64) -> Command {
    let date = format!("{timestamp} +0000");
    let mut command = git.command();
    command
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", date);
    command
}

fn context_with_commit_sha_filter() -> TestEnv {
    TestEnv::new_git().with_filter(r"[a-f0-9]{40}", "[COMMIT_SHA]")
}

/// Helper function to create a local git repository with hooks and incrementing timestamps.
fn create_local_git_repo(context: &TestEnv, repo_name: &str, tags: &[&str]) -> String {
    create_local_git_repo_with_timestamps(context, repo_name, tags, INCREMENTING_STEP_SECS)
}

/// Like `create_local_git_repo`, but all commits and tags share a single fixed timestamp.
/// Simulates mirror repos where all tags are imported simultaneously.
fn create_local_git_repo_fixed_ts(context: &TestEnv, repo_name: &str, tags: &[&str]) -> String {
    create_local_git_repo_with_timestamps(context, repo_name, tags, FIXED_STEP_SECS)
}

fn create_local_git_repo_with_timestamps(
    context: &TestEnv,
    repo_name: &str,
    tags: &[&str],
    timestamp_step_secs: u64,
) -> String {
    let mut timestamp = BASE_TIMESTAMP;
    let tags = tags
        .iter()
        .map(|tag| {
            timestamp += timestamp_step_secs;
            (*tag, timestamp)
        })
        .collect::<Vec<_>>();
    let tip_timestamp = timestamp + timestamp_step_secs;

    create_local_git_repo_with_tag_timestamps(context, repo_name, &tags, tip_timestamp)
}

fn create_local_git_repo_with_tag_ages(
    context: &TestEnv,
    repo_name: &str,
    tags: &[(&str, u64)],
) -> Result<String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs();
    let tags = tags
        .iter()
        .map(|(tag, days_ago)| (*tag, now.saturating_sub(days_ago * 86400)))
        .collect::<Vec<_>>();

    Ok(create_local_git_repo_with_tag_timestamps(
        context, repo_name, &tags, now,
    ))
}

fn create_local_git_repo_with_tag_timestamps(
    context: &TestEnv,
    repo_name: &str,
    tags: &[(&str, u64)],
    tip_timestamp: u64,
) -> String {
    let repo = context.create_repo(repo_name).with_file(
        ".pre-commit-hooks.yaml",
        indoc::indoc! {r#"
            - id: test-hook
              name: Test Hook
              entry: echo
              language: system
            - id: another-hook
              name: Another Hook
              entry: python3 -c 'print("hello")'
              language: python
        "#},
    );
    let initial_timestamp = tags
        .first()
        .map_or(BASE_TIMESTAMP, |(_, timestamp)| timestamp.saturating_sub(1));

    let git = repo.git();
    git.add(".");

    git_command_at(&git, initial_timestamp)
        .arg("commit")
        .arg("-m")
        .arg("Initial commit")
        .assert()
        .success();

    // Create tags
    for (tag, timestamp) in tags {
        git_command_at(&git, *timestamp)
            .arg("commit")
            .arg("-m")
            .arg(format!("Release {tag}"))
            .arg("--allow-empty")
            .assert()
            .success();
        git_command_at(&git, *timestamp)
            .arg("tag")
            .arg(tag)
            .arg("-m")
            .arg(tag)
            .assert()
            .success();
    }

    // Add an extra commit to the tip
    git_command_at(&git, tip_timestamp)
        .arg("commit")
        .arg("-m")
        .arg("tip")
        .arg("--allow-empty")
        .assert()
        .success();

    repo.path().to_string_lossy().replace('\\', "/")
}

#[test]
fn update_basic() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "test-repo", &["v1.0.0", "v1.1.0", "v2.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/test-repo
      updating rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/test-repo
                rev: v2.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_already_up_to_date() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "up-to-date-repo", &["v1.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/up-to-date-repo
                rev: v1.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_cooldown_does_not_downgrade_current_rev() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo_with_tag_ages(
        &context,
        "cooldown-downgrade-repo",
        &[("v0.9.25", 8), ("v0.10.2", 2), ("v0.10.3", 1)],
    )?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v0.10.2
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("7"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/cooldown-downgrade-repo
      keeping current rev `v0.10.2`; 7-day cooldown would select older rev `v0.9.25`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/cooldown-downgrade-repo
                rev: v0.10.2
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_freeze_still_freezes_skipped_cooldown_downgrade() -> Result<()> {
    let context = context_with_commit_sha_filter();

    let repo_path = create_local_git_repo_with_tag_ages(
        &context,
        "freeze-cooldown-downgrade-repo",
        &[("v0.9.25", 8), ("v0.10.2", 2), ("v0.10.3", 1)],
    )?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v0.10.2
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--freeze").arg("--cooldown-days").arg("7"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/freeze-cooldown-downgrade-repo
      updating rev `v0.10.2` -> `[COMMIT_SHA]` (frozen: v0.10.2)

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/freeze-cooldown-downgrade-repo
                rev: [COMMIT_SHA]  # frozen: v0.10.2
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_already_up_to_date_verbose() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "up-to-date-repo-verbose", &["v1.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("-v").arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/up-to-date-repo-verbose
      already up to date at `v1.0.0`

    ----- stderr -----
    ");
}

#[test]
#[cfg(unix)]
fn update_does_not_rewrite_config_when_up_to_date() -> Result<()> {
    use std::time::UNIX_EPOCH;

    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "up-to-date-repo-mtime", &["v1.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    let config_path = context.child(PRE_COMMIT_CONFIG_YAML);

    let before_secs = fs_err::metadata(config_path.path())?
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_secs();

    let assert = context
        .update()
        .arg("--cooldown-days")
        .arg("0")
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout);
    assert!(stdout.is_empty());

    let after_secs = fs_err::metadata(config_path.path())?
        .modified()?
        .duration_since(UNIX_EPOCH)?
        .as_secs();
    assert_eq!(after_secs, before_secs);

    Ok(())
}

#[test]
fn update_multiple_repos_mixed() {
    let context = TestEnv::new_git();

    let repo1_path = create_local_git_repo(&context, "repo1", &["v1.0.0", "v1.1.0"]);
    let repo2_path = create_local_git_repo(&context, "repo2", &["v2.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: missing-hook
          - repo: {}
            rev: v2.0.0
            hooks:
              - id: another-hook
    ", repo1_path, repo1_path, repo2_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    [HOME]/test-repos/repo1
      line 3: updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    [HOME]/test-repos/repo1
      line 7: update failed: Cannot update to rev `v1.1.0`, hook is missing: missing-hook
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/repo1
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/repo1
                rev: v1.0.0
                hooks:
                  - id: missing-hook
              - repo: [HOME]/test-repos/repo2
                rev: v2.0.0
                hooks:
                  - id: another-hook
            ");
}

/// Test that `prek update` ignores the `GIT_DIR` environment variable.
#[test]
fn test_resolve_revision_ignores_git_dir_env_var() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "target-repo", &["v0.1.0", "v0.2.0"]);
    let external_repo_path = create_local_git_repo(&context, "external-repo", &["v9.9.9"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v0.1.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    let mut cmd = context.update();
    cmd.arg("--cooldown-days")
        .arg("0")
        .env("GIT_DIR", ChildPath::new(&external_repo_path).join(".git"));

    cmd_snapshot!(context, cmd, @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/target-repo
      updating rev `v0.1.0` -> `v0.2.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/target-repo
                rev: v0.2.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_specific_repos() {
    let context = TestEnv::new_git();

    let repo1_path = create_local_git_repo(&context, "repo1", &["v1.0.0", "v1.1.0"]);
    let repo2_path = create_local_git_repo(&context, "repo2", &["v2.0.0", "v2.1.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {}
            rev: v2.0.0
            hooks:
              - id: another-hook
    ", repo1_path, repo2_path});

    context.git().add_all();

    // Update only repo1
    cmd_snapshot!(context, context.update().arg("--repo").arg(&repo1_path).arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/repo1
      updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/repo1
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/repo2
                rev: v2.0.0
                hooks:
                  - id: another-hook
            ");

    // Update both repo1 and repo2
    cmd_snapshot!(context, context.update().arg("--repo").arg(&repo1_path).arg("--repo").arg(&repo2_path).arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/repo2
      updating rev `v2.0.0` -> `v2.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/repo1
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/repo2
                rev: v2.1.0
                hooks:
                  - id: another-hook
            ");
}

#[test]
fn update_warns_for_missing_repos() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "configured-repo", &["v1.0.0", "v1.1.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update()
        .arg("--repo").arg(&repo_path)
        .arg("--repo").arg("missing-from-repo")
        .arg("--repo-include-tag").arg(format!("{repo_path}=v*"))
        .arg("--repo-include-tag").arg("missing-from-include=v*")
        .arg("--repo-exclude-tag").arg(format!("{repo_path}=nightly"))
        .arg("--repo-exclude-tag").arg("missing-from-exclude=nightly")
        .arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/configured-repo
      updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    warning: the following repository selectors were not found in the configuration:
      - `--repo=missing-from-repo`
      - `--repo-include-tag=missing-from-include=v*`
      - `--repo-exclude-tag=missing-from-exclude=nightly`
    ");
}

#[test]
fn update_warns_when_repo_override_matches_another_project() {
    let context = TestEnv::new_git();

    let repo1_path = create_local_git_repo(&context, "project-repo-1", &["v1.0.0", "v1.1.0"]);
    let repo2_path = create_local_git_repo(&context, "project-repo-2", &["v1.0.0", "v1.1.0"]);

    context.write_workspace(["project-a", "project-b"], "repos: []");
    let context = context
        .with_file(
            "project-a/.pre-commit-config.yaml",
            indoc::formatdoc! {r#"
        update:
          repos:
            "{}":
              exclude_tags: nightly
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    "#, repo2_path, repo1_path},
        )
        .with_file(
            "project-b/.pre-commit-config.yaml",
            indoc::formatdoc! {r#"
        update:
          repos:
            "{}":
              include_tags: "v*"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    "#, repo1_path, repo2_path},
        );

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--jobs").arg("1"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    project-a/.pre-commit-config.yaml
      [HOME]/test-repos/project-repo-1
        updating rev `v1.0.0` -> `v1.1.0`

    project-b/.pre-commit-config.yaml
      [HOME]/test-repos/project-repo-2
        updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    warning: the following `update.repos` entries were not found in their project configurations:
      `project-a/.pre-commit-config.yaml`:
        - `[HOME]/test-repos/project-repo-2`
      `project-b/.pre-commit-config.yaml`:
        - `[HOME]/test-repos/project-repo-1`
    ");
}

#[test]
fn update_repo_options_match_relative_config_value() {
    let context = TestEnv::new_git();

    let selected_path = create_local_git_repo(
        &context,
        "relative-selected",
        &["v1.0.0", "v1.1.0", "v1.2.0", "v2.0.0"],
    );

    let selected_repo = "../home/test-repos/relative-selected";
    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {selected_repo}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {selected_path}
            rev: v1.0.0
            hooks:
              - id: test-hook
    "});
    context.git().add_all();

    let include_filter = format!("{selected_repo}=v1.*");
    let exclude_filter = format!("{selected_repo}=v1.2.0");
    cmd_snapshot!(context, context.update()
        .arg("--repo").arg(selected_repo)
        .arg("--repo-include-tag").arg(include_filter)
        .arg("--repo-exclude-tag").arg(exclude_filter)
        .arg("--cooldown-days").arg("0"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ../home/test-repos/relative-selected
      updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @r"
            repos:
              - repo: ../home/test-repos/relative-selected
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/relative-selected
                rev: v1.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_exclude_repo_skips_fetching_repo() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "included-repo", &["v1.0.0", "v1.1.0"]);
    let missing_repo_path = context
        .home_dir()
        .child("test-repos/missing-repo")
        .to_string_lossy()
        .to_string();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {}
            rev: v9.9.9
            hooks:
              - id: another-hook
    ", repo_path, missing_repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--exclude-repo").arg(&missing_repo_path).arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/included-repo
      updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/included-repo
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/missing-repo
                rev: v9.9.9
                hooks:
                  - id: another-hook
            ");
}

#[test]
fn update_exclude_repo_matches_relative_config_value() {
    let context = TestEnv::new_git();

    create_local_git_repo(&context, "relative-excluded", &["v1.0.0", "v2.0.0"]);
    create_local_git_repo(&context, "relative-included", &["v1.0.0", "v2.0.0"]);

    let excluded_repo = "../home/test-repos/relative-excluded";
    let included_repo = "../home/test-repos/relative-included";
    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {excluded_repo}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {included_repo}
            rev: v1.0.0
            hooks:
              - id: test-hook
    "});
    context.git().add_all();

    cmd_snapshot!(context, context.update()
        .arg("--exclude-repo").arg(excluded_repo)
        .arg("--cooldown-days").arg("0"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    ../home/test-repos/relative-included
      updating rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    assert_snapshot!(context.read(PRE_COMMIT_CONFIG_YAML), @r"
    repos:
      - repo: ../home/test-repos/relative-excluded
        rev: v1.0.0
        hooks:
          - id: test-hook
      - repo: ../home/test-repos/relative-included
        rev: v2.0.0
        hooks:
          - id: test-hook
    ");
}

#[test]
fn update_tag_filters_include_then_exclude() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "tag-filter-repo",
        &["v1.0.0", "v1.1.0", "v2.0.0", "nightly"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update()
        .arg("--include-tag").arg("v1.*")
        .arg("--include-tag").arg("v2.*")
        .arg("--exclude-tag").arg("v2.*")
        .arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/tag-filter-repo
      updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/tag-filter-repo
                rev: v1.1.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_uses_project_tag_filter_config() {
    let context = TestEnv::new_git();

    let repo1_path = create_local_git_repo(
        &context,
        "tag-filter-config-1",
        &["v1.0.0", "v2.0.0", "v2.1.0", "v3.0.0-rc1"],
    );
    let repo2_path = create_local_git_repo(
        &context,
        "tag-filter-config-2",
        &["v1.0.0", "v2.0.0", "v2.1.0", "v3.0.0-rc1"],
    );

    context.write_config(indoc::formatdoc! {r#"
        update:
          include_tags: "v*"
          exclude_tags: ["*-rc*"]
          repos:
            "{}":
              include_tags: "v2.*"
              exclude_tags: "v2.1.0"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    "#, repo1_path, repo1_path, repo2_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--jobs").arg("1"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/tag-filter-config-1
      updating rev `v1.0.0` -> `v2.0.0`

    [HOME]/test-repos/tag-filter-config-2
      updating rev `v1.0.0` -> `v2.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @r#"
            update:
              include_tags: "v*"
              exclude_tags: ["*-rc*"]
              repos:
                "[HOME]/test-repos/tag-filter-config-1":
                  include_tags: "v2.*"
                  exclude_tags: "v2.1.0"
            repos:
              - repo: [HOME]/test-repos/tag-filter-config-1
                rev: v2.0.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/tag-filter-config-2
                rev: v2.1.0
                hooks:
                  - id: test-hook
            "#);
}

#[test]
fn update_tag_filters_can_select_older_track_without_cooldown() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "tag-filter-older-track",
        &["v1.0.0", "v1.1.0", "v2.0.0"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v2.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update()
        .arg("--include-tag").arg("v1.*")
        .arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/tag-filter-older-track
      updating rev `v2.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/tag-filter-older-track
                rev: v1.1.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_repo_include_tag_is_repo_specific() {
    let context = TestEnv::new_git();

    let repo1_path = create_local_git_repo(
        &context,
        "repo-include-tag-1",
        &["v1.0.0", "v1.1.0", "v2.0.0"],
    );
    let repo2_path = create_local_git_repo(
        &context,
        "repo-include-tag-2",
        &["v1.0.0", "v1.1.0", "v2.0.0"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo1_path, repo2_path});

    context.git().add_all();

    let repo1_filter = format!("{repo1_path}=v1.*");

    cmd_snapshot!(context, context.update()
        .arg("--jobs").arg("1")
        .arg("--repo-include-tag").arg(repo1_filter)
        .arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/repo-include-tag-1
      updating rev `v1.0.0` -> `v1.1.0`

    [HOME]/test-repos/repo-include-tag-2
      updating rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/repo-include-tag-1
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/repo-include-tag-2
                rev: v2.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_repo_include_tag_overrides_global_include_tag() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "repo-include-tag-intersection",
        &["v1.0.0", "v1.1.0", "v2.1.0"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    let repo_filter = format!("{repo_path}=v*.1.0");

    cmd_snapshot!(context, context.update()
        .arg("--include-tag").arg("v1.*")
        .arg("--repo-include-tag").arg(repo_filter)
        .arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/repo-include-tag-intersection
      updating rev `v1.0.0` -> `v2.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/repo-include-tag-intersection
                rev: v2.1.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_repo_exclude_tag_can_leave_repo_unchanged() {
    let context = TestEnv::new_git();

    let repo1_path = create_local_git_repo(&context, "repo-exclude-tag-1", &["v1.0.0", "v2.0.0"]);
    let repo2_path = create_local_git_repo(&context, "repo-exclude-tag-2", &["v1.0.0", "v2.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo1_path, repo2_path});

    context.git().add_all();

    let repo1_filter = format!("{repo1_path}=v2.*");

    cmd_snapshot!(context, context.update()
        .arg("--jobs").arg("1")
        .arg("--include-tag").arg("v2.*")
        .arg("--repo-exclude-tag").arg(repo1_filter)
        .arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/repo-exclude-tag-2
      updating rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/repo-exclude-tag-1
                rev: v1.0.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/repo-exclude-tag-2
                rev: v2.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_bleeding_edge() {
    let context = context_with_commit_sha_filter();

    let repo_path = create_local_git_repo(&context, "bleeding-repo", &["v1.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--bleeding-edge"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/bleeding-repo
      updating rev `v1.0.0` -> `[COMMIT_SHA]`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/bleeding-repo
                rev: [COMMIT_SHA]
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_freeze() {
    let context = context_with_commit_sha_filter();

    let repo_path = create_local_git_repo(&context, "freeze-repo", &["v1.0.0", "v1.1.0"]);
    // Make sure the "# frozen: v1.1.0" comment works correctly by adding a tag without dot
    context
        .git_at(&repo_path)
        .command()
        .arg("tag")
        .arg("v1")
        .arg("-m")
        .arg("v1")
        .arg("v1.1.0^{}")
        .assert()
        .success();

    context.write_config(indoc::formatdoc! {r"
        update:
          freeze: true
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/freeze-repo
      updating rev `v1.0.0` -> `[COMMIT_SHA]` (frozen: v1.1.0)

    ----- stderr -----
    ");

    // Should contain frozen comment
    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            update:
              freeze: true
            repos:
              - repo: [HOME]/test-repos/freeze-repo
                rev: [COMMIT_SHA]  # frozen: v1.1.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_freeze_uses_dereferenced_commit_for_annotated_tags() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "freeze-annotated-repo", &["v1.0.0", "v1.1.0"]);

    let git = context.git_at(&repo_path);
    let tag_object_sha = git.rev_parse("v1.1.0")?;
    let commit_sha = git.rev_parse("v1.1.0^{}")?;

    assert_ne!(
        tag_object_sha, commit_sha,
        "sanity check failed: annotated tag object SHA should differ from commit SHA"
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    context
        .update()
        .arg("--freeze")
        .arg("--cooldown-days")
        .arg("0")
        .assert()
        .success();

    let config = context.read(PRE_COMMIT_CONFIG_YAML);
    assert!(
        config.contains(&format!("rev: {commit_sha}")),
        "expected config to contain the dereferenced commit SHA"
    );
    assert!(
        config.contains("# frozen: v1.1.0"),
        "expected config to preserve the original tag in the frozen comment"
    );
    assert!(
        !config.contains(&tag_object_sha),
        "expected config to not contain the annotated tag object SHA"
    );

    Ok(())
}

#[test]
fn update_shared_target_with_different_frozen_comments_displays_sha() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "shared-target-different-frozen-repo",
        &["v1.0.0", "v1.1.0"],
    );

    context
        .git_at(&repo_path)
        .command()
        .arg("tag")
        .arg("v1")
        .arg("v1.0.0^{}")
        .assert()
        .success();

    let old_commit_sha = context.git_at(&repo_path).rev_parse("v1.0.0^{}")?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.0.0
            hooks:
              - id: test-hook
          - repo: {}
            rev: {}  # frozen: v1
            hooks:
              - id: test-hook
    ", repo_path, old_commit_sha, repo_path, old_commit_sha});

    context.git().add_all();

    let context = context.with_filter(old_commit_sha.clone(), "[OLD_COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/shared-target-different-frozen-repo
      line 3: updating rev `[OLD_COMMIT_SHA]` (frozen: v1.0.0) -> `v1.1.0`
      line 7: updating rev `[OLD_COMMIT_SHA]` (frozen: v1) -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/shared-target-different-frozen-repo
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/shared-target-different-frozen-repo
                rev: v1.1.0
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_preserve_quote_style() {
    let context = TestEnv::new_git();

    let repo1_path = create_local_git_repo(&context, "repo1", &["v1.0.0", "v1.1.0"]);
    let repo2_path = create_local_git_repo(&context, "repo2", &["v1.0.0", "v1.1.0"]);

    // Use specific formatting with comments
    context.write_config(indoc::formatdoc! {r#"
        # Pre-commit configuration
        repos:
          - repo: {}  # Test repository
            rev: v1.0.0  # No quotes
            hooks:
              - id: test-hook
                # Hook configuration
                name: Test Hook
          - repo: {}  # Test repository
            rev: 'v1.0.0'  # Single quotes
            hooks:
              - id: test-hook
                # Hook configuration
                name: Test Hook
          - repo: {}
            rev: "v1.0.0"  # Double quotes
            hooks:
              - id: test-hook
                # Hook configuration
                name: Test Hook
    "#, repo1_path, repo1_path, repo2_path });

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/repo1
      line 4: updating rev `v1.0.0` -> `v1.1.0`
      line 10: updating rev `v1.0.0` -> `v1.1.0`

    [HOME]/test-repos/repo2
      updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @r#"
            # Pre-commit configuration
            repos:
              - repo: [HOME]/test-repos/repo1  # Test repository
                rev: v1.1.0  # No quotes
                hooks:
                  - id: test-hook
                    # Hook configuration
                    name: Test Hook
              - repo: [HOME]/test-repos/repo1  # Test repository
                rev: 'v1.1.0'  # Single quotes
                hooks:
                  - id: test-hook
                    # Hook configuration
                    name: Test Hook
              - repo: [HOME]/test-repos/repo2
                rev: "v1.1.0"  # Double quotes
                hooks:
                  - id: test-hook
                    # Hook configuration
                    name: Test Hook
            "#);
}

#[test]
fn update_with_existing_frozen_comment() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "frozen-repo", &["v1.0.0", "v1.1.0", "v1.2.0"]);

    let commit_sha = "1234567890abcdef1234567890abcdef12345678";

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path, commit_sha});

    context.git().add_all();

    let context = context.with_filter(commit_sha, "[COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/frozen-repo
      updating rev `[COMMIT_SHA]` (frozen: v1.0.0) -> `v1.2.0`

    ----- stderr -----
    warning: [[HOME]/test-repos/frozen-repo] frozen ref `v1.0.0` does not match `[COMMIT_SHA]`
     --> .pre-commit-config.yaml:3:62
      |
    3 |     rev: [COMMIT_SHA]  # frozen: v1.0.0
      |                                                              ^^^^^^ `v1.0.0` resolves to a different commit
      |
      = note: pinned commit `[COMMIT_SHA]` is not present in the repo
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/frozen-repo
                rev: v1.2.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_updates_mismatched_frozen_comment() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "check-frozen-repo", &["v1.0.0", "v1.1.0"]);

    let commit_sha = context.git_at(&repo_path).rev_parse("v1.1.0^{}")?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path, commit_sha});

    context.git().add_all();

    let context = context.with_filter(commit_sha.clone(), "[COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--freeze"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/check-frozen-repo
      updating frozen comment `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    warning: [[HOME]/test-repos/check-frozen-repo] frozen ref `v1.0.0` does not match `[COMMIT_SHA]`
     --> .pre-commit-config.yaml:3:62
      |
    3 |     rev: [COMMIT_SHA]  # frozen: v1.0.0
      |                                                              ^^^^^^ `v1.0.0` resolves to a different commit
      |
      = note: pinned commit `[COMMIT_SHA]` is referenced by `v1.1.0`
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/check-frozen-repo
                rev: [COMMIT_SHA]  # frozen: v1.1.0
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_updates_unresolvable_frozen_comment() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "check-unresolvable-frozen-repo",
        &["v1.0.0", "v1.1.0"],
    );

    let commit_sha = context.git_at(&repo_path).rev_parse("v1.1.0^{}")?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: does-not-exist
            hooks:
              - id: test-hook
    ", repo_path, commit_sha});

    context.git().add_all();

    let context = context.with_filter(commit_sha.clone(), "[COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--freeze"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/check-unresolvable-frozen-repo
      updating frozen comment `does-not-exist` -> `v1.1.0`

    ----- stderr -----
    warning: [[HOME]/test-repos/check-unresolvable-frozen-repo] frozen ref `does-not-exist` does not match `[COMMIT_SHA]`
     --> .pre-commit-config.yaml:3:62
      |
    3 |     rev: [COMMIT_SHA]  # frozen: does-not-exist
      |                                                              ^^^^^^^^^^^^^^ `does-not-exist` could not be resolved
      |
      = note: pinned commit `[COMMIT_SHA]` is referenced by `v1.1.0`
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/check-unresolvable-frozen-repo
                rev: [COMMIT_SHA]  # frozen: v1.1.0
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_removes_frozen_comment_when_pinned_commit_has_no_tag() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "check-remove-frozen-comment-repo",
        &["v1.0.0", "v1.1.0"],
    );

    let commit_sha = context.git_at(&repo_path).rev_parse("HEAD")?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.1.0
            hooks:
              - id: test-hook
    ", repo_path, commit_sha});

    context.git().add_all();

    let context = context.with_filter(commit_sha.clone(), "[COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--bleeding-edge").arg("--freeze"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/check-remove-frozen-comment-repo
      removing frozen comment `v1.1.0`

    ----- stderr -----
    warning: [[HOME]/test-repos/check-remove-frozen-comment-repo] frozen ref `v1.1.0` does not match `[COMMIT_SHA]`
     --> .pre-commit-config.yaml:3:62
      |
    3 |     rev: [COMMIT_SHA]  # frozen: v1.1.0
      |                                                              ^^^^^^ `v1.1.0` resolves to a different commit
      |
      = note: no tag points at the pinned commit `[COMMIT_SHA]`
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/check-remove-frozen-comment-repo
                rev: [COMMIT_SHA]
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_warns_for_branch_only_pinned_commit_with_frozen_comment() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "check-branch-only-pinned-frozen-repo",
        &["v1.0.0", "v1.1.0"],
    );

    context
        .git_at(&repo_path)
        .command()
        .arg("checkout")
        .arg("-b")
        .arg("side")
        .arg("v1.0.0^{}")
        .assert()
        .success();
    context
        .git_at(&repo_path)
        .command()
        .arg("commit")
        .arg("-m")
        .arg("side")
        .arg("--allow-empty")
        .assert()
        .success();
    let branch_commit = context.git_at(&repo_path).rev_parse("HEAD")?;
    context.git_at(&repo_path).checkout("master");

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path, branch_commit});

    context.git().add_all();

    let context = context.with_filter(branch_commit.clone(), "[BRANCH_ONLY_COMMIT]");
    let context = context.with_filter(r"[a-f0-9]{40}", "[COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--freeze").arg("--dry-run"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/check-branch-only-pinned-frozen-repo
      would update rev `[BRANCH_ONLY_COMMIT]` (frozen: v1.0.0) -> `[COMMIT_SHA]` (frozen: v1.1.0)

    ----- stderr -----
    warning: [[HOME]/test-repos/check-branch-only-pinned-frozen-repo] frozen ref `v1.0.0` does not match `[BRANCH_ONLY_COMMIT]`
     --> .pre-commit-config.yaml:3:62
      |
    3 |     rev: [BRANCH_ONLY_COMMIT]  # frozen: v1.0.0
      |                                                              ^^^^^^ `v1.0.0` resolves to a different commit
      |
      = note: pinned commit `[BRANCH_ONLY_COMMIT]` is not present in the repo
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/check-branch-only-pinned-frozen-repo
                rev: [BRANCH_ONLY_COMMIT]  # frozen: v1.0.0
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_warns_for_invalid_pinned_commit_with_frozen_comment() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "check-invalid-pinned-frozen-repo",
        &["v1.0.0", "v1.1.0"],
    );

    let invalid_commit = "1234567890abcdef1234567890abcdef12345678";

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path, invalid_commit});

    context.git().add_all();

    let context = context.with_filters([
        (invalid_commit, "[INVALID_COMMIT]"),
        (r"[a-f0-9]{40}", "[COMMIT_SHA]"),
    ]);

    cmd_snapshot!(context, context.update().arg("--freeze").arg("--dry-run"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/check-invalid-pinned-frozen-repo
      would update rev `[INVALID_COMMIT]` (frozen: v1.0.0) -> `[COMMIT_SHA]` (frozen: v1.1.0)

    ----- stderr -----
    warning: [[HOME]/test-repos/check-invalid-pinned-frozen-repo] frozen ref `v1.0.0` does not match `[INVALID_COMMIT]`
     --> .pre-commit-config.yaml:3:62
      |
    3 |     rev: [INVALID_COMMIT]  # frozen: v1.0.0
      |                                                              ^^^^^^ `v1.0.0` resolves to a different commit
      |
      = note: pinned commit `[INVALID_COMMIT]` is not present in the repo
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/check-invalid-pinned-frozen-repo
                rev: [INVALID_COMMIT]  # frozen: v1.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_dry_run_warns_for_mismatched_frozen_comment() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path =
        create_local_git_repo(&context, "check-frozen-dry-run-repo", &["v1.0.0", "v1.1.0"]);

    let commit_sha = context.git_at(&repo_path).rev_parse("v1.1.0^{}")?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path, commit_sha});

    context.git().add_all();

    let context = context.with_filter(commit_sha.clone(), "[COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--freeze").arg("--dry-run"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/check-frozen-dry-run-repo
      would update frozen comment `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    warning: [[HOME]/test-repos/check-frozen-dry-run-repo] frozen ref `v1.0.0` does not match `[COMMIT_SHA]`
     --> .pre-commit-config.yaml:3:62
      |
    3 |     rev: [COMMIT_SHA]  # frozen: v1.0.0
      |                                                              ^^^^^^ `v1.0.0` resolves to a different commit
      |
      = note: pinned commit `[COMMIT_SHA]` is referenced by `v1.1.0`
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/check-frozen-dry-run-repo
                rev: [COMMIT_SHA]  # frozen: v1.0.0
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_check_fails_for_mismatched_frozen_comment() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path =
        create_local_git_repo(&context, "check-frozen-check-repo", &["v1.0.0", "v1.1.0"]);

    let commit_sha = context.git_at(&repo_path).rev_parse("v1.1.0^{}")?;

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path, commit_sha});

    context.git().add_all();

    let context = context.with_filter(commit_sha.clone(), "[COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--freeze").arg("--check"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    [HOME]/test-repos/check-frozen-check-repo
      would update frozen comment `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    warning: [[HOME]/test-repos/check-frozen-check-repo] frozen ref `v1.0.0` does not match `[COMMIT_SHA]`
     --> .pre-commit-config.yaml:3:62
      |
    3 |     rev: [COMMIT_SHA]  # frozen: v1.0.0
      |                                                              ^^^^^^ `v1.0.0` resolves to a different commit
      |
      = note: pinned commit `[COMMIT_SHA]` is referenced by `v1.1.0`
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/check-frozen-check-repo
                rev: [COMMIT_SHA]  # frozen: v1.0.0
                hooks:
                  - id: test-hook
            ");

    Ok(())
}

#[test]
fn update_updates_mismatched_frozen_comment_toml() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path =
        create_local_git_repo(&context, "check-frozen-repo-toml", &["v1.0.0", "v1.1.0"]);

    let commit_sha = context.git_at(&repo_path).rev_parse("v1.1.0^{}")?;

    let context = context.with_file(
        PREK_TOML,
        indoc::formatdoc! {r#"
        [[repos]]
        repo = "{}"
        rev = "{}" # frozen: v1.0.0
        hooks = [
          {{ id = "test-hook" }},
        ]
        "#, repo_path, commit_sha},
    );

    context.git().add_all();

    let context = context.with_filter(commit_sha.clone(), "[COMMIT_SHA]");

    cmd_snapshot!(context, context.update().arg("--freeze"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/check-frozen-repo-toml
      updating frozen comment `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    warning: [[HOME]/test-repos/check-frozen-repo-toml] frozen ref `v1.0.0` does not match `[COMMIT_SHA]`
     --> prek.toml:3:60
      |
    3 | rev = "[COMMIT_SHA]" # frozen: v1.0.0
      |                                                            ^^^^^^ `v1.0.0` resolves to a different commit
      |
      = note: pinned commit `[COMMIT_SHA]` is referenced by `v1.1.0`
    "#);

    snapshot!(context, context.read(PREK_TOML), @r#"
            [[repos]]
            repo = "[HOME]/test-repos/check-frozen-repo-toml"
            rev = "[COMMIT_SHA]" # frozen: v1.1.0
            hooks = [
              { id = "test-hook" },
            ]
            "#);

    Ok(())
}

#[test]
fn update_local_repo_ignored() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "remote-repo", &["v1.0.0", "v1.1.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local-hook
                name: Local Hook
                language: system
                entry: echo
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/remote-repo
      updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: local
                hooks:
                  - id: local-hook
                    name: Local Hook
                    language: system
                    entry: echo
              - repo: [HOME]/test-repos/remote-repo
                rev: v1.1.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn missing_hook_ids() -> Result<()> {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "missing-hook-repo", &["v1.0.0"]);

    // Remove the 'test-hook' from the hooks file
    ChildPath::new(&repo_path)
        .child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r#"
        - id: another-hook
          name: Another Hook
          entry: python3 -c 'print("hello")'
          language: python
    "#})?;

    context
        .git_at(&repo_path)
        .add(".")
        .commit("Remove test-hook")
        .tag("v2.0.0");

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    [HOME]/test-repos/missing-hook-repo
      update failed: Cannot update to rev `v2.0.0`, hook is missing: test-hook
    ");

    Ok(())
}

#[test]
fn update_workspace() {
    let context = TestEnv::new_git();

    let repo1_path =
        create_local_git_repo(&context, "workspace-repo1", &["v1.0.0", "v1.1.0", "v2.0.0"]);
    let repo2_path = create_local_git_repo(&context, "workspace-repo2", &["v1.0.0", "v1.5.0"]);
    let repo3_path = create_local_git_repo(&context, "workspace-repo3", &["v2.0.0"]);

    context.write_workspace(
        ["project-a", "project-b"],
        "repos: []", // Minimal valid config for root
    );

    let context = context
        .with_file(
            "project-a/.pre-commit-config.yaml",
            indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: another-hook
    ", repo1_path, repo2_path},
        )
        .with_file(
            "project-b/.pre-commit-config.yaml",
            indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: another-hook
          - repo: {}
            rev: v2.0.0
            hooks:
              - id: test-hook
    ", repo2_path, repo3_path},
        );

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    project-a/.pre-commit-config.yaml
      [HOME]/test-repos/workspace-repo1
        updating rev `v1.0.0` -> `v2.0.0`

      [HOME]/test-repos/workspace-repo2
        updating rev `v1.0.0` -> `v1.5.0`

    project-b/.pre-commit-config.yaml
      [HOME]/test-repos/workspace-repo2
        updating rev `v1.0.0` -> `v1.5.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read("project-a/.pre-commit-config.yaml"), @"
            repos:
              - repo: [HOME]/test-repos/workspace-repo1
                rev: v2.0.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/workspace-repo2
                rev: v1.5.0
                hooks:
                  - id: another-hook
            ");

    snapshot!(context, context.read("project-b/.pre-commit-config.yaml"), @"
            repos:
              - repo: [HOME]/test-repos/workspace-repo2
                rev: v1.5.0
                hooks:
                  - id: another-hook
              - repo: [HOME]/test-repos/workspace-repo3
                rev: v2.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_workspace_same_repo_uses_project_cooldown() {
    let context = TestEnv::new_git();
    context.write_user_config(indoc::indoc! {r"
        [update]
        cooldown_days = 1
    "});

    let repo_path =
        create_local_git_repo(&context, "workspace-cooldown-repo", &["v1.0.0", "v1.1.0"]);
    context
        .git_at(&repo_path)
        .command()
        .arg("commit")
        .arg("-m")
        .arg("Release v2.0.0")
        .arg("--allow-empty")
        .assert()
        .success();
    context.git_at(&repo_path).tag("v2.0.0");

    context.write_workspace(
        ["project-a", "project-b"],
        "repos: []", // Minimal valid config for root
    );

    let context = context
        .with_file(
            "project-a/.pre-commit-config.yaml",
            indoc::formatdoc! {r"
        update:
          cooldown_days: 0
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path},
        )
        .with_file(
            "project-b/.pre-commit-config.yaml",
            indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path},
        );

    context.git().add_all();

    cmd_snapshot!(context, context.update(), @"
    success: true
    exit_code: 0
    ----- stdout -----
    project-a/.pre-commit-config.yaml
      [HOME]/test-repos/workspace-cooldown-repo
        updating rev `v1.0.0` -> `v2.0.0`

    project-b/.pre-commit-config.yaml
      [HOME]/test-repos/workspace-cooldown-repo
        updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read("project-a/.pre-commit-config.yaml"), @"
            update:
              cooldown_days: 0
            repos:
              - repo: [HOME]/test-repos/workspace-cooldown-repo
                rev: v2.0.0
                hooks:
                  - id: test-hook
            ");

    snapshot!(context, context.read("project-b/.pre-commit-config.yaml"), @"
            repos:
              - repo: [HOME]/test-repos/workspace-cooldown-repo
                rev: v1.1.0
                hooks:
                  - id: test-hook
            ");
}

// When multiple tags point to the same object, prek prefers a tag that:
// - contains a dot (e.g., a SemVer-like tag), and
// - is most similar to the current revision, as measured by Levenshtein distance.
#[test]
fn prefer_similar_tags() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "remote-repo", &["v1.0.0", "v1.1.0"]);
    // Add a second tag (`foo-v1.1.0`) pointing at the same commit as `v1.1.0`.
    // From the current `rev` (`v1.0.0`):
    // - `levenshtein(v1.0.0, v1.1.0) == 1`
    // - `levenshtein(v1.0.0, foo-v1.1.0) == 5`
    // Therefore, `v1.1.0` should be selected as the update target.
    // But if the newest SemVer-like tag (e.g v1.1.111111) were less similar than `foo-v1.1.0`, we would select `foo-v1.1.0` instead.
    context
        .git_at(&repo_path)
        .command()
        .arg("tag")
        .arg("foo-v1.1.0")
        .arg("-m")
        .arg("foo-v1.1.0")
        .arg("v1.1.0^{}")
        .assert()
        .success();
    // Add tag v1 pointing to the same commit as v1.1.0
    context
        .git_at(&repo_path)
        .command()
        .arg("tag")
        .arg("v1")
        .arg("-m")
        .arg("v1")
        .arg("v1.1.0^{}")
        .assert()
        .success();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: local
            hooks:
              - id: local-hook
                name: Local Hook
                language: system
                entry: echo
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/remote-repo
      updating rev `v1.0.0` -> `v1.1.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: local
                hooks:
                  - id: local-hook
                    name: Local Hook
                    language: system
                    entry: echo
              - repo: [HOME]/test-repos/remote-repo
                rev: v1.1.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_dry_run() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "test-repo", &["v1.0.0", "v1.1.0", "v2.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--dry-run").arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/test-repo
      would update rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/test-repo
                rev: v1.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_check() {
    let context = TestEnv::new_git();

    let repo_path =
        create_local_git_repo(&context, "check-test-repo", &["v1.0.0", "v1.1.0", "v2.0.0"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--check").arg("--cooldown-days").arg("0"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    [HOME]/test-repos/check-test-repo
      would update rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/check-test-repo
                rev: v1.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_dry_run_exit_code() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "dry-run-exit-code-test-repo",
        &["v1.0.0", "v1.1.0", "v2.0.0"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--dry-run").arg("--exit-code").arg("--cooldown-days").arg("0"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    [HOME]/test-repos/dry-run-exit-code-test-repo
      would update rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/dry-run-exit-code-test-repo
                rev: v1.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_exit_code_updates_config() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "exit-code-test-repo",
        &["v1.0.0", "v1.1.0", "v2.0.0"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--exit-code").arg("--cooldown-days").arg("0"), @"
    success: false
    exit_code: 1
    ----- stdout -----
    [HOME]/test-repos/exit-code-test-repo
      updating rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/exit-code-test-repo
                rev: v2.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_exit_code_succeeds_when_up_to_date() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(
        &context,
        "exit-code-up-to-date-test-repo",
        &["v1.0.0", "v2.0.0"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v2.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--exit-code").arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/exit-code-up-to-date-test-repo
                rev: v2.0.0
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn quoting_float_like_version_number() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "test-repo", &["0.49", "0.50"]);

    // Our serializer will quote these float-like strings by default. Use a different
    // quoting style here to validate that explicit quotes are still preserved.
    context.write_config(indoc::formatdoc! {r#"
        repos:
          - repo: {}
            rev: "0.49"
            hooks:
              - id: test-hook
    "#, repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/test-repo
      updating rev `0.49` -> `0.50`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @r#"
            repos:
              - repo: [HOME]/test-repos/test-repo
                rev: "0.50"
                hooks:
                  - id: test-hook
            "#);
}

#[test]
fn quoting_float_like_version_number_without_existing_quotes() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo(&context, "test-repo", &["v0.19", "0.51"]);

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v0.19
            hooks:
              - id: test-hook
    ", repo_path});
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/test-repo
      updating rev `v0.19` -> `0.51`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @r#"
            repos:
              - repo: [HOME]/test-repos/test-repo
                rev: "0.51"
                hooks:
                  - id: test-hook
            "#);
}

#[test]
fn update_with_invalid_config_file() {
    let context =
        TestEnv::new_git().with_file(PRE_COMMIT_CONFIG_YAML, "invalid_yaml: [unclosed_list");

    cmd_snapshot!(context, context.update(), @"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to parse `.pre-commit-config.yaml`
      caused by: error: line 1 column 15: unclosed bracket '['
     --> <input>:1:15
      |
    1 | invalid_yaml: [unclosed_list
      |               ^ unclosed bracket '['
    ");
}

#[test]
fn update_toml() {
    let context = TestEnv::new_git();

    let repo_path =
        create_local_git_repo(&context, "test-repo-toml", &["v1.0.0", "v1.1.0", "v2.0.0"]);

    let context = context.with_file(
        PREK_TOML,
        indoc::formatdoc! {r#"
        [[repos]]
        repo = "{}"
        rev = "v1.0.0"
        hooks = [
          {{ id = "test-hook" }},
        ]
      "#, repo_path},
    );
    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/test-repo-toml
      updating rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PREK_TOML), @r#"
        [[repos]]
        repo = "[HOME]/test-repos/test-repo-toml"
        rev = "v2.0.0"
        hooks = [
          { id = "test-hook" },
        ]
        "#);
}

#[test]
fn update_toml_with_comment() {
    let context = TestEnv::new_git();

    let repo_path =
        create_local_git_repo(&context, "test-repo-toml", &["v1.0.0", "v1.1.0", "v2.0.0"]);

    let context = context.with_file(
        PREK_TOML,
        indoc::formatdoc! {r#"
        [[repos]]
        repo = "{}"
        rev = "v1.0.0" # This is a comment
        hooks = [
          {{ id = "test-hook" }},
        ]
      "#, repo_path},
    );

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/test-repo-toml
      updating rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PREK_TOML), @r#"
        [[repos]]
        repo = "[HOME]/test-repos/test-repo-toml"
        rev = "v2.0.0" # This is a comment
        hooks = [
          { id = "test-hook" },
        ]
        "#);

    // "frozen: xx" comment should be removed
    context.write_file(
        PREK_TOML,
        indoc::formatdoc! {r#"
        [[repos]]
        repo = "{}"
        rev = "v1.0.0" # frozen: v1.0.0
        hooks = [
          {{ id = "test-hook" }},
        ]
      "#, repo_path},
    );

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/test-repo-toml
      updating rev `v1.0.0` (frozen: v1.0.0) -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PREK_TOML), @r#"
        [[repos]]
        repo = "[HOME]/test-repos/test-repo-toml"
        rev = "v2.0.0"
        hooks = [
          { id = "test-hook" },
        ]
        "#);
}

#[test]
fn update_freeze_toml() {
    let context = context_with_commit_sha_filter();

    let repo_path = create_local_git_repo(&context, "freeze-repo", &["v1.0.0", "v1.1.0"]);
    context.write_user_config(indoc::indoc! {r"
        [update]
        freeze = true
    "});
    // Make sure the "# frozen: v1.1.0" comment works correctly by adding a tag without dot
    context
        .git_at(&repo_path)
        .command()
        .arg("tag")
        .arg("v1")
        .arg("-m")
        .arg("v1")
        .arg("v1.1.0^{}")
        .assert()
        .success();

    let context = context.with_file(
        PREK_TOML,
        indoc::formatdoc! {r#"
        [[repos]]
        repo = "{}"
        rev = "v1.0.0"
        hooks = [
          {{ id = "test-hook" }},
        ]
    "#, repo_path},
    );

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/freeze-repo
      updating rev `v1.0.0` -> `[COMMIT_SHA]` (frozen: v1.1.0)

    ----- stderr -----
    ");

    // Should contain frozen comment
    snapshot!(context, context.read(PREK_TOML), @r#"
            [[repos]]
            repo = "[HOME]/test-repos/freeze-repo"
            rev = "[COMMIT_SHA]"  # frozen: v1.1.0
            hooks = [
              { id = "test-hook" },
            ]
            "#);
}

#[test]
fn update_equal_timestamp_tags_picks_highest_version() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo_fixed_ts(
        &context,
        "mirror-repo",
        &["v1.0.0", "v1.0.1", "v1.0.2", "v1.0.3", "v1.0.4", "v1.0.5"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.3
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/mirror-repo
      updating rev `v1.0.3` -> `v1.0.5`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/mirror-repo
                rev: v1.0.5
                hooks:
                  - id: test-hook
            ");
}

// When all tags share a timestamp and some are non-semver (e.g. "latest", "stable"),
// semver tags should be preferred and sorted highest-first.
#[test]
fn update_equal_timestamp_prefers_semver_over_nonsemver() {
    let context = TestEnv::new_git();

    let repo_path = create_local_git_repo_fixed_ts(
        &context,
        "mixed-tags-repo",
        &["v1.0.0", "latest", "v2.0.0", "stable"],
    );

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/mixed-tags-repo
      updating rev `v1.0.0` -> `v2.0.0`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/mixed-tags-repo
                rev: v2.0.0
                hooks:
                  - id: test-hook
            ");
}

// When tags span multiple timestamp groups, the newest group should be selected first.
// Within an equal-timestamp group, semver tiebreaker picks the highest version.
#[test]
fn update_mixed_timestamps_with_equal_subgroups() {
    let context = TestEnv::new_git();

    // Create base repo with v1.0.x tags at incrementing timestamps.
    let repo_path = create_local_git_repo(&context, "mixed-ts-repo", &["v1.0.0", "v1.0.1"]);

    // Add a second group of tags sharing a single newer timestamp
    // (must be in the past so the cooldown filter doesn't exclude them).
    let newer_ts = "1500000000 +0000";
    for tag in &["v2.0.1", "v2.0.0"] {
        context
            .git_at(&repo_path)
            .command()
            .arg("commit")
            .arg("-m")
            .arg(format!("Release {tag}"))
            .arg("--allow-empty")
            .env("GIT_AUTHOR_DATE", newer_ts)
            .env("GIT_COMMITTER_DATE", newer_ts)
            .assert()
            .success();
        context
            .git_at(&repo_path)
            .command()
            .arg("tag")
            .arg(tag)
            .arg("-m")
            .arg(tag)
            .env("GIT_AUTHOR_DATE", newer_ts)
            .env("GIT_COMMITTER_DATE", newer_ts)
            .assert()
            .success();
    }

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/mixed-ts-repo
      updating rev `v1.0.0` -> `v2.0.1`

    ----- stderr -----
    ");

    snapshot!(context, context.read(PRE_COMMIT_CONFIG_YAML), @"
            repos:
              - repo: [HOME]/test-repos/mixed-ts-repo
                rev: v2.0.1
                hooks:
                  - id: test-hook
            ");
}

#[test]
fn update_freeze_toml_with_comment() {
    let context = context_with_commit_sha_filter();

    let repo_path = create_local_git_repo(&context, "freeze-repo", &["v1.0.0", "v1.1.0"]);
    // Make sure the "# frozen: v1.1.0" comment works correctly by adding a tag without dot
    context
        .git_at(&repo_path)
        .command()
        .arg("tag")
        .arg("v1")
        .arg("-m")
        .arg("v1")
        .arg("v1.1.0^{}")
        .assert()
        .success();

    let context = context.with_file(
        PREK_TOML,
        indoc::formatdoc! {r#"
        [[repos]]
        repo = "{}"
        # A comment above
        rev = "v1.0.0" # This is a comment
        # A comment below
        hooks = [
          {{ id = "test-hook" }},
        ]
    "#, repo_path},
    );

    context.git().add_all();

    cmd_snapshot!(context, context.update().arg("--freeze").arg("--cooldown-days").arg("0"), @"
    success: true
    exit_code: 0
    ----- stdout -----
    [HOME]/test-repos/freeze-repo
      updating rev `v1.0.0` -> `[COMMIT_SHA]` (frozen: v1.1.0)

    ----- stderr -----
    ");

    // Should contain frozen comment
    snapshot!(context, context.read(PREK_TOML), @r#"
            [[repos]]
            repo = "[HOME]/test-repos/freeze-repo"
            # A comment above
            rev = "[COMMIT_SHA]" # frozen: v1.1.0
            # A comment below
            hooks = [
              { id = "test-hook" },
            ]
            "#);
}
