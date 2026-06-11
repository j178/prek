use assert_cmd::assert::OutputAssertExt;
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
use prek_consts::PRE_COMMIT_HOOKS_YAML;
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

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

    Ok(())
}

#[test]
fn remote_repo_install() -> anyhow::Result<()> {
    let hook_repo = TestContext::new();
    hook_repo.init_project();

    hook_repo
        .work_dir()
        .child(PRE_COMMIT_HOOKS_YAML)
        .write_str(indoc::indoc! {r"
            - id: hello
              name: hello
              language: perl
              entry: perl -MPrek::Hello -e 'Prek::Hello::hello()'
        "})?;

    hook_repo
        .work_dir()
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
        .work_dir()
        .child("lib")
        .child("Prek")
        .create_dir_all()?;
    hook_repo
        .work_dir()
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

    hook_repo.git_add(".");
    hook_repo.git_commit("Add perl hook");
    hook_repo.git_tag("v1.0.0");

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(&indoc::formatdoc! {r"
        repos:
          - repo: {}
            rev: v1.0.0
            hooks:
              - id: hello
                always_run: true
                verbose: true
                pass_filenames: false
    ", hook_repo.work_dir().display()});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().env(EnvVars::HOME, &**context.home_dir()), @r"
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
