use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestEnv, cmd_snapshot};

#[test]
fn local_hook() -> anyhow::Result<()> {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run().env(EnvVars::HOME, &**context.home_dir()), @r"
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

#[test]
fn remote_repo_install() -> anyhow::Result<()> {
    let context = TestEnv::new_git();
    let hook_repo = context.create_repo("perl-hook");

    hook_repo
        .path()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
            - id: hello
              name: hello
              language: perl
              entry: perl -MPrek::Hello -e 'Prek::Hello::hello()'
        "})?;

    hook_repo
        .path()
        .child("Makefile.PL")
        .write_str(indoc::indoc! {r"
            use strict;
            use warnings;
            use ExtUtils::MakeMaker;

            WriteMakefile(
                NAME => 'Prek::Hello',
                VERSION_FROM => 'lib/Prek/Hello.pm',
            );
        "})?;

    hook_repo
        .path()
        .child("lib")
        .child("Prek")
        .create_dir_all()?;
    hook_repo
        .path()
        .child("lib")
        .child("Prek")
        .child("Hello.pm")
        .write_str(indoc::indoc! {r#"
            package Prek::Hello;

            use strict;
            use warnings;

            our $VERSION = '0.01';

            sub hello {
                print "Hello from remote Perl!\n";
            }

            1;
        "#})?;

    hook_repo
        .git()
        .add_all()
        .commit("Add perl hook")
        .tag("v1.0.0");

    let context = context.with_config(indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: hello
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo.path().display()});

    context.git().add_all();

    cmd_snapshot!(context, context.run().env(EnvVars::HOME, &**context.home_dir()), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello....................................................................Passed
    - hook id: hello
    - duration: [TIME]

      Hello from remote Perl!

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn additional_dependencies() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
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

    context.git().add_all();

    context
        .run()
        .env(EnvVars::HOME, &**context.home_dir())
        .assert()
        .stdout(predicates::str::contains("This is perltidy, v20211029"));
}

#[test]
fn language_version() {
    let context = TestEnv::new_git().with_config(indoc::indoc! {r"
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

    context.git().add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Failed to init hooks
      caused by: Invalid hook `local`
      caused by: Hook specified `language_version: 5.34` but the language `perl` does not support toolchain installation for now
    ");
}
