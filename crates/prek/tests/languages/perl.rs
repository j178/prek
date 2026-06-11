#[cfg(unix)]
use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteStr, PathChild};
#[cfg(unix)]
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

#[cfg(unix)]
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
                language: perl
                entry: perl hello.pl
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context
        .work_dir()
        .child("hello.pl")
        .write_str(indoc::indoc! {r#"
            use strict;
            use warnings;

            print "Hello from Perl!\n";
        "#})?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().env(EnvVars::HOME, &**context.home_dir()), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello from Perl!

    ----- stderr -----
    ");

    cmd_snapshot!(context.filters(), context.run().env(EnvVars::HOME, &**context.home_dir()), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello from Perl!

    ----- stderr -----
    ");

    Ok(())
}

#[cfg(unix)]
#[test]
fn additional_dependencies() -> anyhow::Result<()> {
    if !EnvVars::is_set(EnvVars::CI) {
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: perltidy
                name: perltidy
                language: perl
                entry: perltidy --version
                additional_dependencies: [SHANCOCK/Perl-Tidy-20211029.tar.gz]
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    context
        .run()
        .env(EnvVars::HOME, &**context.home_dir())
        .assert()
        .stdout(predicates::str::contains("This is perltidy, v20211029"));

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
              - id: local
                name: local
                language: perl
                entry: perl -v
                language_version: '5.34'
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `local`
      caused by: Hook specified `language_version: 5.34` but the language `perl` does not support toolchain installation for now
    ");
}

#[cfg(unix)]
#[test]
fn shell_hook() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: perl-shell
                name: perl-shell
                language: perl
                entry: |
                  perl -e 'print "shell args: @ARGV\n"' "$@"
                shell: sh
                args: [configured]
                verbose: true
    "#});

    context.work_dir().child("input.txt").write_str("input")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().env(EnvVars::HOME, &**context.home_dir()), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    perl-shell...............................................................Passed
    - hook id: perl-shell
    - duration: [TIME]

      shell args: configured input.txt .pre-commit-config.yaml

    ----- stderr -----
    ");

    Ok(())
}
