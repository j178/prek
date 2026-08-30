use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn local_hook() {
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .with_file("hello.php", "<?php echo \"Hello from PHP!\\n\";\n")
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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
    cmd_snapshot!(context, context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello from PHP!

    ----- stderr -----
    ");
}

#[test]
fn remote_repo_install() -> anyhow::Result<()> {
    let context = TestEnv::new().init_git();
    let hook_repo = context
        .create_hook_repo(
            "php-hook",
            indoc::indoc! {r"
                - id: php-hook
                  name: php-hook
                  language: php
                  entry: php-hook
            "},
        )
        .with_file(
            COMPOSER_JSON,
            indoc::indoc! {r#"
                {
                  "name": "prek-test/php-hook",
                  "bin": ["bin/php-hook"]
                }
            "#},
        )
        .with_executable_file(
            "bin/php-hook",
            indoc::indoc! {r#"
        #!/usr/bin/env php
        <?php echo "Hello from remote PHP!\n";
    "#},
        )
        .build();

    context.write_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: php-hook
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo});
    context.git().add(".");

    let composer_home = context.home_dir().child("composer");
    composer_home.create_dir_all()?;
    cmd_snapshot!(context,
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
    let dependency = TestEnv::new()
        .with_file(
            COMPOSER_JSON,
            indoc::indoc! {r#"
                {
                  "name": "prek-test/php-dependency",
                  "bin": ["bin/php-dependency"]
                }
            "#},
        )
        .with_executable_file(
            "bin/php-dependency",
            indoc::indoc! {r#"
                #!/usr/bin/env php
                <?php echo "Hello from an additional dependency!\n";
            "#},
        );

    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

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

    cmd_snapshot!(context,
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
    let context = TestEnv::new()
        .with_config(indoc::indoc! {r"
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
    "})
        .init_git();

    cmd_snapshot!(context, context.run(), @r"
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
