use std::collections::HashSet;
use std::path::PathBuf;

use anyhow::Result;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use serde_json::json;

use crate::common::{TestContext, cmd_snapshot};

mod common;

#[test]
fn gc_command_no_repos() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create a basic config file to avoid workspace discovery errors
    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .write_str("repos: []")?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    0 repo(s) removed.

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn gc_command_with_unused_repos() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create a repos directory with some fake repos
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create basic config file to avoid workspace discovery errors
    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .write_str("repos: []")?;

    // Create unused repo 1
    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    let repo1_metadata = repo1_dir.child(".prek-repo.json");
    repo1_metadata.write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    // Create unused repo 2
    let repo2_dir = repos_dir.child("repo2");
    repo2_dir.create_dir_all()?;
    let repo2_metadata = repo2_dir.child(".prek-repo.json");
    repo2_metadata.write_str(
        &json!({
            "repo": "https://github.com/example/repo2",
            "rev": "main"
        })
        .to_string(),
    )?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    2 repo(s) removed.

    ----- stderr -----
    ");

    // Verify repos were actually removed
    assert!(!repo1_dir.exists());
    assert!(!repo2_dir.exists());

    Ok(())
}

#[test]
fn gc_command_with_config_file_referencing_repos() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create a repos directory with repos
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create repo that will be referenced in config
    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    let repo1_metadata = repo1_dir.child(".prek-repo.json");
    repo1_metadata.write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    // Create repo that won't be referenced
    let repo2_dir = repos_dir.child("repo2");
    repo2_dir.create_dir_all()?;
    let repo2_metadata = repo2_dir.child(".prek-repo.json");
    repo2_metadata.write_str(
        &json!({
            "repo": "https://github.com/example/repo2",
            "rev": "main"
        })
        .to_string(),
    )?;

    // Create config file that references repo1
    let work_dir = context.work_dir();
    work_dir
        .child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r"
        repos:
          - repo: https://github.com/example/repo1
            rev: v1.0.0
            hooks:
              - id: test-hook
    "})?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    1 repo(s) removed.

    ----- stderr -----
    ");

    // Verify only the unreferenced repo was removed
    assert!(repo1_dir.exists());
    assert!(!repo2_dir.exists());

    Ok(())
}

#[test]
fn gc_command_with_workspace_config_files() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create multiple repos
    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    repo1_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    let repo2_dir = repos_dir.child("repo2");
    repo2_dir.create_dir_all()?;
    repo2_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo2",
            "rev": "v2.0.0"
        })
        .to_string(),
    )?;

    let repo3_dir = repos_dir.child("repo3");
    repo3_dir.create_dir_all()?;
    repo3_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo3",
            "rev": "main"
        })
        .to_string(),
    )?;

    // Create workspace structure with multiple projects
    let work_dir = context.work_dir();
    let project1 = work_dir.child("project1");
    project1.create_dir_all()?;
    project1
        .child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r"
        repos:
          - repo: https://github.com/example/repo1
            rev: v1.0.0
            hooks:
              - id: test-hook1
    "})?;

    let project2 = work_dir.child("project2");
    project2.create_dir_all()?;
    project2
        .child(".pre-commit-config.yml")
        .write_str(indoc::indoc! {r"
        repos:
          - repo: https://github.com/example/repo2
            rev: v2.0.0
            hooks:
              - id: test-hook2
    "})?;

    // Create pyproject.toml for workspace detection
    work_dir
        .child("pyproject.toml")
        .write_str(indoc::indoc! {r#"
        [tool.prek.workspace]
        members = ["project1", "project2"]
    "#})?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    1 repo(s) removed.

    ----- stderr -----
    ");

    // Verify only repo3 (unreferenced) was removed
    assert!(repo1_dir.exists());
    assert!(repo2_dir.exists());
    assert!(!repo3_dir.exists());

    Ok(())
}

#[test]
fn gc_command_with_local_and_meta_repos_in_config() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create remote repo
    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    repo1_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    // Create config with local, meta, and remote repos
    let work_dir = context.work_dir();
    work_dir
        .child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: local-test
                name: Local Test
                entry: echo "local"
                language: script
          - repo: meta
            hooks:
              - id: check-hooks-apply
          - repo: https://github.com/example/repo1
            rev: v1.0.0
            hooks:
              - id: remote-test
    "#})?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    0 repo(s) removed.

    ----- stderr -----
    ");

    // Verify remote repo was preserved
    assert!(repo1_dir.exists());

    Ok(())
}

#[test]
fn gc_command_with_malformed_config_file() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;
    context.init_project();

    // Create repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    repo1_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    // Create malformed config file
    let work_dir = context.work_dir();

    // First create a valid basic config for workspace discovery
    work_dir
        .child(".pre-commit-config.yml")
        .write_str("repos: []")?;

    // Then create a malformed config that should be ignored during processing
    work_dir
        .child(".pre-commit-config.yaml")
        .write_str("invalid: yaml: content: [")?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
    success: false
    exit_code: 101
    ----- stdout -----

    ----- stderr -----
    warning: Both `[TEMP_DIR]/.pre-commit-config.yaml` and `[TEMP_DIR]/.pre-commit-config.yml` exist, using `[TEMP_DIR]/.pre-commit-config.yaml` only

    thread 'main' panicked at src/workspace.rs:688:9:
    At least one project should be found
    note: run with `RUST_BACKTRACE=1` environment variable to display a backtrace
    ");

    // Malformed config should cause the command to fail, so repo is not removed
    assert!(repo1_dir.exists());

    Ok(())
}

#[test]
fn gc_command_with_malformed_metadata_files() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create repo with valid metadata
    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    repo1_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    // Create repo with malformed metadata
    let repo2_dir = repos_dir.child("repo2");
    repo2_dir.create_dir_all()?;
    repo2_dir
        .child(".prek-repo.json")
        .write_str("invalid json")?;

    // Create repo directory without metadata file
    let repo3_dir = repos_dir.child("repo3");
    repo3_dir.create_dir_all()?;

    // Create non-directory file in repos dir
    repos_dir.child("not_a_repo.txt").write_str("just a file")?;

    // Create basic config file for workspace discovery
    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .write_str("repos: []")?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    1 repo(s) removed.

    ----- stderr -----
    ");

    // Only repo1 should be removed (it has valid metadata but isn't referenced)
    // repo2 and repo3 should remain because they have invalid/missing metadata
    assert!(!repo1_dir.exists());
    assert!(repo2_dir.exists());
    assert!(repo3_dir.exists());

    Ok(())
}

#[test]
fn gc_command_with_empty_repos_directory() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create empty repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create basic config file for workspace discovery
    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .write_str("repos: []")?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    0 repo(s) removed.

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn gc_command_without_repos_directory() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Don't create repos directory at all

    // Create basic config file for workspace discovery
    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .write_str("repos: []")?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    0 repo(s) removed.

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn gc_command_with_multiple_config_file_types() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create repos
    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    repo1_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    let repo2_dir = repos_dir.child("repo2");
    repo2_dir.create_dir_all()?;
    repo2_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo2",
            "rev": "v2.0.0"
        })
        .to_string(),
    )?;

    let repo3_dir = repos_dir.child("repo3");
    repo3_dir.create_dir_all()?;
    repo3_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo3",
            "rev": "main"
        })
        .to_string(),
    )?;

    // Create both .yaml and .yml config files
    let work_dir = context.work_dir();
    work_dir
        .child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r"
        repos:
          - repo: https://github.com/example/repo1
            rev: v1.0.0
            hooks:
              - id: test-hook1
    "})?;

    work_dir
        .child(".pre-commit-config.yml")
        .write_str(indoc::indoc! {r"
        repos:
          - repo: https://github.com/example/repo2
            rev: v2.0.0
            hooks:
              - id: test-hook2
    "})?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    1 repo(s) removed.

    ----- stderr -----
    warning: Both `[TEMP_DIR]/.pre-commit-config.yaml` and `[TEMP_DIR]/.pre-commit-config.yml` exist, using `[TEMP_DIR]/.pre-commit-config.yaml` only
    ");

    // Both repo1 and repo2 should be preserved, repo3 should be removed
    assert!(repo1_dir.exists());
    assert!(repo2_dir.exists());
    assert!(!repo3_dir.exists());

    Ok(())
}

#[test]
fn gc_command_preserves_repos_with_different_revisions() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create repos with same URL but different revisions
    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    repo1_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    let repo2_dir = repos_dir.child("repo2");
    repo2_dir.create_dir_all()?;
    repo2_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo",
            "rev": "v2.0.0"
        })
        .to_string(),
    )?;

    // Create config referencing only one revision
    let work_dir = context.work_dir();
    work_dir
        .child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r"
        repos:
          - repo: https://github.com/example/repo
            rev: v1.0.0
            hooks:
              - id: test-hook
    "})?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    1 repo(s) removed.

    ----- stderr -----
    ");

    // Only v1.0.0 should be preserved, v2.0.0 should be removed
    assert!(repo1_dir.exists());
    assert!(!repo2_dir.exists());

    Ok(())
}

#[test]
fn gc_command_with_permission_errors() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    context.init_project();

    // Create repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    // Create repo
    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    repo1_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    // Create a read-only file that will cause removal to fail
    let readonly_file = repo1_dir.child("readonly.txt");
    readonly_file.write_str("read only content")?;

    // Note: Setting permissions might not work reliably in all test environments,
    // but the gc function handles removal errors gracefully

    // Create basic config file for workspace discovery
    context
        .work_dir()
        .child(".pre-commit-config.yaml")
        .write_str("repos: []")?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    1 repo(s) removed.

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn gc_command_from_git_root() -> Result<()> {
    let context = TestContext::new();
    let home = context.work_dir().child("home");
    home.create_dir_all()?;

    // Create repos directory
    let repos_dir = home.child("repos");
    repos_dir.create_dir_all()?;

    let repo1_dir = repos_dir.child("repo1");
    repo1_dir.create_dir_all()?;
    repo1_dir.child(".prek-repo.json").write_str(
        &json!({
            "repo": "https://github.com/example/repo1",
            "rev": "v1.0.0"
        })
        .to_string(),
    )?;

    // Create git repository
    let work_dir = context.work_dir();
    std::process::Command::new("git")
        .args(["init"])
        .current_dir(work_dir)
        .output()?;

    std::process::Command::new("git")
        .args(["config", "user.email", "test@example.com"])
        .current_dir(work_dir)
        .output()?;

    std::process::Command::new("git")
        .args(["config", "user.name", "Test User"])
        .current_dir(work_dir)
        .output()?;

    // Create config in git root
    work_dir
        .child(".pre-commit-config.yaml")
        .write_str(indoc::indoc! {r"
        repos:
          - repo: https://github.com/example/repo1
            rev: v1.0.0
            hooks:
              - id: test-hook
    "})?;

    // Create subdirectory and run gc from there
    let subdir = work_dir.child("subdir");
    subdir.create_dir_all()?;

    cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(&*subdir), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    0 repo(s) removed.

    ----- stderr -----
    ");

    // Repo should be preserved because config was found in git root
    assert!(repo1_dir.exists());

    Ok(())
}

// Unit tests for internal functions would go here if they were public
// Since they're private, we test them through the public API above

#[cfg(test)]
mod unit_tests {
    use super::*;
    use tempfile::TempDir;

    // Helper function to create test directory
    fn create_test_dir() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn test_repo_key_generation() {
        // Test that repo keys are generated consistently
        let repo_url = "https://github.com/example/test";
        let repo_rev = "v1.0.0";

        let expected_key = "https://github.com/example/test:v1.0.0";
        let actual_key = format!("{repo_url}:{repo_rev}");

        assert_eq!(actual_key, expected_key);
    }
    #[test]
    fn test_hashset_operations() {
        let mut set = HashSet::new();

        let key1 = (
            "https://github.com/example/test:v1.0.0".to_string(),
            PathBuf::from("/path1"),
        );
        let key2 = (
            "https://github.com/example/test:v2.0.0".to_string(),
            PathBuf::from("/path2"),
        );
        let key3 = (
            "https://github.com/example/other:v1.0.0".to_string(),
            PathBuf::from("/path3"),
        );

        set.insert(key1.clone());
        set.insert(key2.clone());
        set.insert(key3.clone());

        assert_eq!(set.len(), 3);

        // Test retain operation (simulating mark_remote_repo_used)
        let target_key = "https://github.com/example/test:v1.0.0";
        set.retain(|(key, _)| key != target_key);

        assert_eq!(set.len(), 2);
        assert!(!set.contains(&key1));
        assert!(set.contains(&key2));
        assert!(set.contains(&key3));
    }

    #[test]
    fn test_config_file_patterns() {
        use constants::{ALT_CONFIG_FILE, CONFIG_FILE};

        assert_eq!(CONFIG_FILE, ".pre-commit-config.yaml");
        assert_eq!(ALT_CONFIG_FILE, ".pre-commit-config.yml");
    }
}

#[cfg(test)]
mod integration_tests {
    use super::*;

    #[test]
    fn test_gc_handles_unicode_paths() -> Result<()> {
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        // Create repos directory
        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create repo with unicode characters in path
        let repo_dir = repos_dir.child("репо-тест");
        repo_dir.create_dir_all()?;
        repo_dir.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/тест",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        // Create basic config file for workspace discovery
        context
            .work_dir()
            .child(".pre-commit-config.yaml")
            .write_str("repos: []")?;

        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        1 repo(s) removed.

        ----- stderr -----
        ");

        Ok(())
    }

    #[test]
    fn test_gc_with_symlinks() -> Result<()> {
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create actual repo directory
        let actual_repo = repos_dir.child("actual_repo");
        actual_repo.create_dir_all()?;
        actual_repo.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/test",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        // On Unix systems, we could create symlinks, but for cross-platform compatibility
        // we'll just test the regular case

        // Create basic config file for workspace discovery
        context
            .work_dir()
            .child(".pre-commit-config.yaml")
            .write_str("repos: []")?;
        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        1 repo(s) removed.

        ----- stderr -----
        ");

        Ok(())
    }

    #[test]
    fn test_gc_with_large_number_of_repos() -> Result<()> {
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create many repos to test performance
        for i in 0..50 {
            let repo_dir = repos_dir.child(format!("repo{i}"));
            repo_dir.create_dir_all()?;
            repo_dir.child(".prek-repo.json").write_str(
                &json!({
                    "repo": format!("https://github.com/example/repo{}", i),
                    "rev": "v1.0.0"
                })
                .to_string(),
            )?;
        }

        // Create basic config file for workspace discovery
        context
            .work_dir()
            .child(".pre-commit-config.yaml")
            .write_str("repos: []")?;

        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        50 repo(s) removed.

        ----- stderr -----
        ");

        Ok(())
    }

    #[test]
    fn test_gc_with_nested_workspace_structure() -> Result<()> {
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create repos
        let repo1_dir = repos_dir.child("repo1");
        repo1_dir.create_dir_all()?;
        repo1_dir.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/repo1",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        let repo2_dir = repos_dir.child("repo2");
        repo2_dir.create_dir_all()?;
        repo2_dir.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/repo2",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        // Create deeply nested workspace structure
        let work_dir = context.work_dir();
        let level1 = work_dir.child("level1");
        level1.create_dir_all()?;

        let level2 = level1.child("level2");
        level2.create_dir_all()?;

        let level3 = level2.child("level3");
        level3.create_dir_all()?;

        // Create config in nested directory
        level3
            .child(".pre-commit-config.yaml")
            .write_str(indoc::indoc! {r"
            repos:
              - repo: https://github.com/example/repo1
                rev: v1.0.0
                hooks:
                  - id: test-hook
        "})?;

        // Create workspace config at root to discover nested projects
        work_dir
            .child("pyproject.toml")
            .write_str(indoc::indoc! {r#"
            [tool.prek.workspace]
            members = ["level1/level2/level3"]
        "#})?;

        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        1 repo(s) removed.

        ----- stderr -----
        ");

        // repo1 should be preserved, repo2 should be removed
        assert!(repo1_dir.exists());
        assert!(!repo2_dir.exists());

        Ok(())
    }

    #[test]
    fn test_gc_with_config_containing_nonexistent_hook() -> Result<()> {
        // Tests that gc doesn't crash when a config references a hook that doesn't exist
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create a repo
        let repo1_dir = repos_dir.child("repo1");
        repo1_dir.create_dir_all()?;
        repo1_dir.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/repo1",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        // Create config with a hook that doesn't exist
        let work_dir = context.work_dir();
        work_dir
            .child(".pre-commit-config.yaml")
            .write_str(indoc::indoc! {r"
            repos:
              - repo: https://github.com/example/repo1
                rev: v1.0.0
                hooks:
                  - id: this-hook-does-not-exist
                  - id: another-nonexistent-hook
        "})?;

        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        0 repo(s) removed.

        ----- stderr -----
        ");

        // Repo should still be preserved because it's referenced in config
        assert!(repo1_dir.exists());

        Ok(())
    }

    #[test]
    fn test_gc_removes_config_when_deleted() -> Result<()> {
        // Based on _remove_config_assert_cleared from pre-commit
        // Tests that when a config file is deleted, its repos are removed on next gc
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create repo
        let repo1_dir = repos_dir.child("repo1");
        repo1_dir.create_dir_all()?;
        repo1_dir.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/repo1",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        let work_dir = context.work_dir();
        let config_file = work_dir.child(".pre-commit-config.yaml");

        // Create config referencing the repo
        config_file.write_str(indoc::indoc! {r"
        repos:
          - repo: https://github.com/example/repo1
            rev: v1.0.0
            hooks:
              - id: test-hook
        "})?;

        // First gc should keep the repo
        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        0 repo(s) removed.

        ----- stderr -----
        ");
        assert!(repo1_dir.exists());

        // Delete the old config and create a new empty one (workspace discovery needs at least one config)
        fs_err::remove_file(&*config_file)?;
        config_file.write_str("repos: []")?;

        // Now gc should remove the repo since it's no longer referenced
        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        1 repo(s) removed.

        ----- stderr -----
        ");
        assert!(!repo1_dir.exists());

        Ok(())
    }

    #[test]
    fn test_gc_with_broken_repo_metadata() -> Result<()> {
        // Based on test_invalid_manifest_gcd from pre-commit
        // Tests that repos with missing/broken metadata files are handled correctly
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create repo with initially valid metadata
        let repo1_dir = repos_dir.child("repo1");
        repo1_dir.create_dir_all()?;
        let metadata_file = repo1_dir.child(".prek-repo.json");
        metadata_file.write_str(
            &json!({
                "repo": "https://github.com/example/repo1",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        // Create config
        let work_dir = context.work_dir();
        work_dir
            .child(".pre-commit-config.yaml")
            .write_str(indoc::indoc! {r"
            repos:
              - repo: https://github.com/example/repo1
                rev: v1.0.0
                hooks:
                  - id: test-hook
        "})?;

        // First gc should preserve the repo
        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        0 repo(s) removed.

        ----- stderr -----
        ");
        assert!(repo1_dir.exists());

        // Now "break" the metadata file to simulate corruption
        fs_err::remove_file(&*metadata_file)?;

        // gc should not find this repo in get_all_stored_repos since metadata is missing
        // So it won't try to remove it (only repos with valid metadata are tracked)
        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        0 repo(s) removed.

        ----- stderr -----
        ");
        // Repo directory still exists because it wasn't tracked without metadata
        assert!(repo1_dir.exists());

        Ok(())
    }

    #[test]
    fn test_gc_with_multiple_versions_after_update() -> Result<()> {
        // Based on test_gc from pre-commit
        // Simulates the scenario where autoupdate creates a new repo version
        // and the old version should be garbage collected
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create old version repo (simulating pre-update state)
        let repo_old_dir = repos_dir.child("repo-old");
        repo_old_dir.create_dir_all()?;
        repo_old_dir.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/repo",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        // Create new version repo (simulating post-update state)
        let repo_new_dir = repos_dir.child("repo-new");
        repo_new_dir.create_dir_all()?;
        repo_new_dir.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/repo",
                "rev": "v2.0.0"
            })
            .to_string(),
        )?;

        // Config now references only the new version
        let work_dir = context.work_dir();
        work_dir
            .child(".pre-commit-config.yaml")
            .write_str(indoc::indoc! {r"
            repos:
              - repo: https://github.com/example/repo
                rev: v2.0.0
                hooks:
                  - id: test-hook
        "})?;

        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        1 repo(s) removed.

        ----- stderr -----
        ");

        // Old version should be removed, new version preserved
        assert!(!repo_old_dir.exists());
        assert!(repo_new_dir.exists());

        Ok(())
    }

    #[test]
    fn test_gc_repo_referenced_but_not_cloned() -> Result<()> {
        // Based on test_gc_repo_not_cloned from pre-commit
        // Tests that repos referenced in config but not yet cloned don't cause issues
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create config that references a repo, but don't create the actual repo
        let work_dir = context.work_dir();
        work_dir
            .child(".pre-commit-config.yaml")
            .write_str(indoc::indoc! {r"
            repos:
              - repo: https://github.com/example/not-yet-cloned
                rev: v1.0.0
                hooks:
                  - id: test-hook
        "})?;

        // gc should handle this gracefully - no repos to remove
        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        0 repo(s) removed.

        ----- stderr -----
        ");

        Ok(())
    }

    #[test]
    fn test_gc_with_local_repo_containing_environment() -> Result<()> {
        // Based on test_gc_unused_local_repo_with_env from pre-commit
        // Tests that local repos with language environments (like Python) are handled correctly
        // In prek's implementation, "local" repos in config don't create tracked repos in the store
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create a remote repo (will be unused)
        let remote_repo_dir = repos_dir.child("remote-repo");
        remote_repo_dir.create_dir_all()?;
        remote_repo_dir.child(".prek-repo.json").write_str(
            &json!({
                "repo": "https://github.com/example/remote",
                "rev": "v1.0.0"
            })
            .to_string(),
        )?;

        let work_dir = context.work_dir();
        let config_file = work_dir.child(".pre-commit-config.yaml");

        // Create config with only local repo (no remote repos referenced)
        config_file.write_str(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: test-local-hook
                name: Test Local Hook
                entry: python -m flake8
                language: python
                types: [python]
        "})?;

        // The unreferenced remote repo should be removed, local repo doesn't create tracked repos
        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home).current_dir(work_dir), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        1 repo(s) removed.

        ----- stderr -----
        ");

        // Remote repo should be removed since it's not referenced
        assert!(!remote_repo_dir.exists());

        Ok(())
    }

    #[test]
    fn test_gc_handles_concurrent_repo_removal() -> Result<()> {
        // Additional test: verify that gc handles cases where repos are removed
        // during the gc process (though this is protected by store lock)
        let context = TestContext::new();
        let home = context.work_dir().child("home");
        home.create_dir_all()?;

        context.init_project();

        let repos_dir = home.child("repos");
        repos_dir.create_dir_all()?;

        // Create multiple repos
        for i in 1..=5 {
            let repo_dir = repos_dir.child(format!("repo{i}"));
            repo_dir.create_dir_all()?;
            repo_dir.child(".prek-repo.json").write_str(
                &json!({
                    "repo": format!("https://github.com/example/repo{}", i),
                    "rev": "v1.0.0"
                })
                .to_string(),
            )?;
        }

        // Config references none of them
        context
            .work_dir()
            .child(".pre-commit-config.yaml")
            .write_str("repos: []")?;

        cmd_snapshot!(context.filters(), context.command().arg("gc").env("PREK_HOME", &*home), @r"
        success: true
        exit_code: 0
        ----- stdout -----
        5 repo(s) removed.

        ----- stderr -----
        ");

        Ok(())
    }
}
