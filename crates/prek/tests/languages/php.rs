use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;

use crate::common::{TestContext, cmd_snapshot, make_executable};

#[test]
fn local_hook() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: hello
                name: hello
                language: php
                entry: php hello.php
                always_run: true
                verbose: true
                pass_filenames: false
    "});
    context
        .work_dir()
        .child("hello.php")
        .write_str("<?php echo \"Hello from PHP!\\n\";\n")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello from PHP!

    ----- stderr -----
    ");

    // The second run reuses the environment and checks the recorded PHP executable and version.
    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello from PHP!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn remote_repo_install() -> anyhow::Result<()> {
    let hook_repo = TestContext::new();
    hook_repo.init_project();

    hook_repo
        .work_dir()
        .child(COMPOSER_JSON)
        .write_str(indoc::indoc! {r#"
            {
              "name": "prek-test/php-hook",
              "bin": ["bin/php-hook"]
            }
        "#})?;
    hook_repo
        .work_dir()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
            - id: php-hook
              name: php-hook
              language: php
              entry: php-hook
        "})?;
    hook_repo.work_dir().child("bin").create_dir_all()?;
    let hook_binary = hook_repo.work_dir().child("bin/php-hook");
    hook_binary.write_str(indoc::indoc! {r#"
        #!/usr/bin/env php
        <?php echo "Hello from remote PHP!\n";
    "#})?;
    make_executable(hook_binary.path())?;

    hook_repo.git_add(".");
    hook_repo.git_commit("Add PHP hook");
    hook_repo.git_tag("v1.0.0");

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: php-hook
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo.work_dir().display()});
    context.git_add(".");

    let composer_home = context.home_dir().child("composer");
    composer_home.create_dir_all()?;
    cmd_snapshot!(
        context.filters(),
        context.run().env("COMPOSER_HOME", composer_home.path()),
        @r"
    success: true
    exit_code: 0
    ----- stdout -----
    php-hook.................................................................Passed
    - hook id: php-hook
    - duration: [TIME]

      Hello from remote PHP!

    ----- stderr -----
    "
    );

    Ok(())
}

#[test]
fn additional_dependencies() -> anyhow::Result<()> {
    let dependency = TestContext::new();
    dependency
        .work_dir()
        .child(COMPOSER_JSON)
        .write_str(indoc::indoc! {r#"
            {
              "name": "prek-test/php-dependency",
              "bin": ["bin/php-dependency"]
            }
        "#})?;
    dependency.work_dir().child("bin").create_dir_all()?;
    let dependency_binary = dependency.work_dir().child("bin/php-dependency");
    dependency_binary.write_str(indoc::indoc! {r#"
        #!/usr/bin/env php
        <?php echo "Hello from an additional dependency!\n";
    "#})?;
    make_executable(dependency_binary.path())?;

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: php-dependency
                name: php-dependency
                language: php
                entry: php-dependency
                additional_dependencies: [prek-test/php-dependency:dev-prek]
                always_run: true
                verbose: true
                pass_filenames: false
    "});
    context.git_add(".");

    let composer_home = context.home_dir().child("composer");
    composer_home.create_dir_all()?;
    composer_home
        .child("config.json")
        .write_str(&serde_json::to_string_pretty(&serde_json::json!({
            "repositories": [{
                "type": "path",
                "url": dependency.work_dir().to_string_lossy(),
                "options": {
                    "symlink": false,
                    "versions": {
                        "prek-test/php-dependency": "dev-prek",
                    },
                },
            }],
        }))?)?;

    cmd_snapshot!(
        context.filters(),
        context.run().env("COMPOSER_HOME", composer_home.path()),
        @r"
    success: true
    exit_code: 0
    ----- stdout -----
    php-dependency...........................................................Passed
    - hook id: php-dependency
    - duration: [TIME]

      Hello from an additional dependency!

    ----- stderr -----
    "
    );

    Ok(())
}

#[test]
fn language_version() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: php
                name: php
                language: php
                entry: php --version
                language_version: '8.4'
                always_run: true
                pass_filenames: false
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `php`
      caused by: Hook specified `language_version: 8.4` but the language `php` does not support toolchain installation for now
    ");
}

const COMPOSER_JSON: &str = "composer.json";
