use std::process::Command;

use anyhow::Result;
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::ChildPath;
use assert_fs::prelude::*;
use insta::assert_snapshot;

use crate::common::{TestContext, cmd_snapshot};

mod common;

/// Helper function to create a local git repository with hooks
fn create_local_git_repo(context: &TestContext, repo_name: &str, tags: &[&str]) -> Result<String> {
    let repo_dir = context.home_dir().child(format!("test-repos/{repo_name}"));
    repo_dir.create_dir_all()?;

    Command::new("git")
        .arg("init")
        .current_dir(&repo_dir)
        .assert()
        .success();
    Command::new("git")
        .arg("config")
        .arg("user.name")
        .arg("Prek Test")
        .current_dir(&repo_dir)
        .assert()
        .success();
    Command::new("git")
        .arg("config")
        .arg("user.email")
        .arg("test@prek.dev")
        .current_dir(&repo_dir)
        .assert()
        .success();
    Command::new("git")
        .arg("config")
        .arg("core.autocrlf")
        .arg("false")
        .current_dir(&repo_dir)
        .assert()
        .success();

    // Create .pre-commit-hooks.yaml
    repo_dir
        .child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r#"
        - id: test-hook
          name: Test Hook
          entry: echo
          language: system
        - id: another-hook
          name: Another Hook
          entry: python3 -c 'print("hello")'
          language: python
    "#})?;

    Command::new("git")
        .arg("add")
        .arg(".")
        .current_dir(&repo_dir)
        .assert()
        .success();

    Command::new("git")
        .arg("commit")
        .arg("-m")
        .arg("Initial commit")
        .current_dir(&repo_dir)
        .assert()
        .success();

    // Create tags
    for tag in tags {
        Command::new("git")
            .arg("commit")
            .arg("-m")
            .arg(format!("Release {tag}"))
            .arg("--allow-empty")
            .current_dir(&repo_dir)
            .assert()
            .success();
        Command::new("git")
            .arg("tag")
            .arg(tag)
            .arg("-m")
            .arg(tag)
            .current_dir(&repo_dir)
            .assert()
            .success();
    }

    // Add an extra commit to the tip
    Command::new("git")
        .arg("commit")
        .arg("-m")
        .arg("tip")
        .arg("--allow-empty")
        .current_dir(&repo_dir)
        .assert()
        .success();

    Ok(repo_dir.to_string_lossy().to_string())
}

#[test]
fn auto_update_basic() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo_path = create_local_git_repo(&context, "test-repo", &["v1.0.0", "v1.1.0", "v2.0.0"])?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git_add(".");

    let filters = context.filters();

    cmd_snapshot!(filters.clone(), context.auto_update(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/test-repo] updating v1.0.0 -> v2.0.0

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r#"
            repos:
              - repo: [HOME]/test-repos/test-repo
                rev: v2.0.0
                hooks:
                  - id: test-hook
            "#);
        }
    );

    Ok(())
}

#[test]
fn auto_update_already_up_to_date() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo_path = create_local_git_repo(&context, "up-to-date-repo", &["v1.0.0"])?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git_add(".");

    let filters = context.filters();

    cmd_snapshot!(filters.clone(), context.auto_update(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/up-to-date-repo] already up to date

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r#"
            repos:
              - repo: [HOME]/test-repos/up-to-date-repo
                rev: v1.0.0
                hooks:
                  - id: test-hook
            "#);
        }
    );

    Ok(())
}

#[test]
fn auto_update_multiple_repos_mixed() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo1_path = create_local_git_repo(&context, "repo1", &["v1.0.0", "v1.1.0"])?;
    let repo2_path = create_local_git_repo(&context, "repo2", &["v2.0.0"])?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
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

    context.git_add(".");

    let filters = context.filters();

    cmd_snapshot!(filters.clone(), context.auto_update(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/repo1] updating v1.0.0 -> v1.1.0
    [[HOME]/test-repos/repo2] already up to date

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r#"
            repos:
              - repo: [HOME]/test-repos/repo1
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/repo2
                rev: v2.0.0
                hooks:
                  - id: another-hook
            "#);
        }
    );

    Ok(())
}

#[test]
fn auto_update_specific_repos() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo1_path = create_local_git_repo(&context, "repo1", &["v1.0.0", "v1.1.0"])?;
    let repo2_path = create_local_git_repo(&context, "repo2", &["v2.0.0", "v2.1.0"])?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
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

    context.git_add(".");

    let filters = context.filters();

    // Update only repo1
    cmd_snapshot!(filters.clone(), context.auto_update().arg("--repo").arg(&repo1_path), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/repo1] updating v1.0.0 -> v1.1.0

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r#"
            repos:
              - repo: [HOME]/test-repos/repo1
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/repo2
                rev: v2.0.0
                hooks:
                  - id: another-hook
            "#);
        }
    );

    // Update both repo1 and repo2
    cmd_snapshot!(filters.clone(), context.auto_update().arg("--repo").arg(&repo1_path).arg("--repo").arg(&repo2_path), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/repo1] already up to date
    [[HOME]/test-repos/repo2] updating v2.0.0 -> v2.1.0

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r#"
            repos:
              - repo: [HOME]/test-repos/repo1
                rev: v1.1.0
                hooks:
                  - id: test-hook
              - repo: [HOME]/test-repos/repo2
                rev: v2.1.0
                hooks:
                  - id: another-hook
            "#);
        }
    );

    Ok(())
}

#[test]
fn auto_update_bleeding_edge() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo_path = create_local_git_repo(&context, "bleeding-repo", &["v1.0.0"])?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([("[a-f0-9]{40}", "[COMMIT_SHA]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters.clone(), context.auto_update().arg("--bleeding-edge"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/bleeding-repo] updating v1.0.0 -> [COMMIT_SHA]

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r#"
            repos:
              - repo: [HOME]/test-repos/bleeding-repo
                rev: [COMMIT_SHA]
                hooks:
                  - id: test-hook
            "#);
        }
    );

    Ok(())
}

#[test]
fn auto_update_freeze() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo_path = create_local_git_repo(&context, "freeze-repo", &["v1.0.0", "v1.1.0"])?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(r" [a-f0-9]{40}", r" [COMMIT_SHA]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters.clone(), context.auto_update().arg("--freeze"), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/freeze-repo] updating v1.0.0 -> [COMMIT_SHA]

    ----- stderr -----
    "#);

    // Should contain frozen comment
    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r##"
            repos:
              - repo: [HOME]/test-repos/freeze-repo
                rev: [COMMIT_SHA]  # frozen: v1.1.0
                hooks:
                  - id: test-hook
            "##);
        }
    );

    Ok(())
}

#[test]
fn auto_update_preserve_formatting() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo1_path = create_local_git_repo(&context, "repo1", &["v1.0.0", "v1.1.0"])?;
    let repo2_path = create_local_git_repo(&context, "repo2", &["v1.0.0", "v1.1.0"])?;

    // Use specific formatting with comments
    context.write_pre_commit_config(&indoc::formatdoc! {r#"
        # Pre-commit configuration
        repos:
          - repo: {}  # Test repository
            rev: 'v1.0.0'  # Current version
            hooks:
              - id: test-hook
                # Hook configuration
                name: Test Hook
          - repo: {}
            rev: "v1.0.0"  # Current version
            hooks:
              - id: test-hook
                # Hook configuration
                name: Test Hook
    "#, repo1_path, repo2_path });

    context.git_add(".");

    let filters = context.filters();

    cmd_snapshot!(filters.clone(), context.auto_update(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/repo1] updating v1.0.0 -> v1.1.0
    [[HOME]/test-repos/repo2] updating v1.0.0 -> v1.1.0

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r##"
            # Pre-commit configuration
            repos:
              - repo: [HOME]/test-repos/repo1  # Test repository
                rev: 'v1.1.0'  # Current version
                hooks:
                  - id: test-hook
                    # Hook configuration
                    name: Test Hook
              - repo: [HOME]/test-repos/repo2
                rev: "v1.1.0"  # Current version
                hooks:
                  - id: test-hook
                    # Hook configuration
                    name: Test Hook
            "##);
        }
    );

    Ok(())
}

#[test]
fn auto_update_with_existing_frozen_comment() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo_path =
        create_local_git_repo(&context, "frozen-repo", &["v1.0.0", "v1.1.0", "v1.2.0"])?;

    let commit_sha = "1234567890abcdef1234567890abcdef12345678";

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: {}  # frozen: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path, commit_sha});

    context.git_add(".");

    let filters = context
        .filters()
        .into_iter()
        .chain([(commit_sha, "[COMMIT_SHA]")])
        .collect::<Vec<_>>();

    cmd_snapshot!(filters.clone(), context.auto_update(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/frozen-repo] updating [COMMIT_SHA] -> v1.2.0

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r#"
            repos:
              - repo: [HOME]/test-repos/frozen-repo
                rev: v1.2.0
                hooks:
                  - id: test-hook
            "#);
        }
    );

    Ok(())
}

#[test]
fn auto_update_local_repo_ignored() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo_path = create_local_git_repo(&context, "remote-repo", &["v1.0.0", "v1.1.0"])?;

    context.write_pre_commit_config(&indoc::formatdoc! {r"
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

    context.git_add(".");

    let filters = context.filters();

    cmd_snapshot!(filters.clone(), context.auto_update(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    [[HOME]/test-repos/remote-repo] updating v1.0.0 -> v1.1.0

    ----- stderr -----
    "#);

    insta::with_settings!(
        { filters => filters.clone() },
        {
            assert_snapshot!(context.read(".pre-commit-config.yaml"), @r#"
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
            "#);
        }
    );

    Ok(())
}

#[test]
fn missing_hook_ids() -> Result<()> {
    let context = TestContext::new();
    context.init_project();

    let repo_path = create_local_git_repo(&context, "missing-hook-repo", &["v1.0.0"])?;

    // Remove the 'test-hook' from the hooks file
    ChildPath::new(&repo_path)
        .child(".pre-commit-hooks.yaml")
        .write_str(indoc::indoc! {r#"
        - id: another-hook
          name: Another Hook
          entry: python3 -c 'print("hello")'
          language: python
    "#})?;

    Command::new("git")
        .arg("add")
        .arg(".")
        .current_dir(&repo_path)
        .assert()
        .success();
    Command::new("git")
        .arg("commit")
        .arg("-m")
        .arg("Remove test-hook")
        .current_dir(&repo_path)
        .assert()
        .success();
    Command::new("git")
        .arg("tag")
        .arg("v2.0.0")
        .arg("-m")
        .arg("v2.0.0")
        .current_dir(&repo_path)
        .assert()
        .success();

    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: test-hook
    ", repo_path});
    context.git_add(".");

    let filters = context.filters();

    cmd_snapshot!(filters.clone(), context.auto_update(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----

    ----- stderr -----
    [[HOME]/test-repos/missing-hook-repo] update failed: Cannot update to rev `v2.0.0`, hook is missing: test-hook
    "#);

    Ok(())
}
