#[cfg(unix)]
use std::path::Path;

#[cfg(unix)]
use assert_cmd::assert::OutputAssertExt;
#[cfg(unix)]
use assert_fs::fixture::{FileWriteStr, PathChild, PathCreateDir};
#[cfg(unix)]
use prek_consts::env_vars::EnvVars;

use crate::common::{TestContext, cmd_snapshot};

#[cfg(unix)]
fn make_executable(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let metadata = fs_err::metadata(path)?;
    let mut permissions = metadata.permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs_err::set_permissions(path, permissions)?;
    Ok(())
}

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
                entry: hello-perl
                always_run: true
                verbose: true
                pass_filenames: false
    "});

    context
        .work_dir()
        .child("Makefile.PL")
        .write_str(indoc::indoc! {r"
            use strict;
            use warnings;
            use ExtUtils::MakeMaker;

            WriteMakefile(
                NAME => 'Prek::Hello',
                VERSION_FROM => 'lib/Prek/Hello.pm',
                EXE_FILES => ['bin/hello-perl'],
            );
        "})?;

    context.work_dir().child("bin").create_dir_all()?;
    context
        .work_dir()
        .child("bin")
        .child("hello-perl")
        .write_str(indoc::indoc! {r"
            #!/usr/bin/env perl
            use strict;
            use warnings;
            use Prek::Hello;

            Prek::Hello::hello();
        "})?;
    make_executable(context.work_dir().child("bin").child("hello-perl").path())?;

    context
        .work_dir()
        .child("lib")
        .child("Prek")
        .create_dir_all()?;
    context
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
                print "Hello from Perl!\n";
            }

            1;
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

    context
        .work_dir()
        .child("Makefile.PL")
        .write_str(indoc::indoc! {r"
            use strict;
            use warnings;
            use ExtUtils::MakeMaker;

            WriteMakefile(
                NAME => 'Prek::PerlTidy',
                VERSION => '0.01',
            );
        "})?;

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

    context
        .work_dir()
        .child("Makefile.PL")
        .write_str(indoc::indoc! {r"
            use ExtUtils::MakeMaker;
            WriteMakefile(
                NAME => 'Prek::Shell',
                VERSION => '0.01',
            );
        "})?;
    context.work_dir().child("input.txt").write_str("input")?;

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run().env(EnvVars::HOME, &**context.home_dir()), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    perl-shell...............................................................Passed
    - hook id: perl-shell
    - duration: [TIME]

      shell args: configured Makefile.PL .pre-commit-config.yaml input.txt

    ----- stderr -----
    ");

    Ok(())
}
