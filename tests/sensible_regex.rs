mod common;

use crate::common::{cmd_snapshot, TestContext};

#[test]
fn sensible_regex_warnings() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        files: 'src/*\.py'
        exclude: 'src/[\\/]_vendor/.*'
        repos:
          - repo: local
            hooks:
              - id: hook-1
                name: Hook 1
                entry: echo
                language: system
                files: 'tests[/\\]'
              - id: hook-2
                name: Hook 2
                entry: echo
                language: system
                exclude: 'lib[\/]'
    "#});
    context.git_add(".");

    // `run` will trigger config parsing and the warnings.
    // The hooks themselves won't run on any files, which is fine.
    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Hook 1...............................................(no files to check)Skipped
    Hook 2...............................................(no files to check)Skipped

    ----- stderr -----
    warning: The top-level `files` field in `.pre-commit-config.yaml` is a regex, not a glob -- matching '/*' probably isn't what you want here
    warning: prek normalizes slashes in the top-level `exclude` field in `.pre-commit-config.yaml` to forward slashes, so you can use `/` instead of `[\\/]`
    warning: prek normalizes slashes in the hook `hook-1` `files` field in `.pre-commit-config.yaml` to forward slashes, so you can use `/` instead of `[/\\]`
    warning: prek normalizes slashes in the hook `hook-2` `exclude` field in `.pre-commit-config.yaml` to forward slashes, so you can use `/` instead of `[\/]`
    "#);
}

#[test]
fn no_sensible_regex_warnings() {
    let context = TestContext::new();
    context.init_project();

    context.write_pre_commit_config(indoc::indoc! {r#"
        files: 'src/.*\.py'
        exclude: 'src/_vendor/.*'
        repos:
          - repo: local
            hooks:
              - id: hook-1
                name: Hook 1
                entry: echo
                language: system
                files: 'tests/'
              - id: hook-2
                name: Hook 2
                entry: echo
                language: system
                exclude: 'lib/'
    "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    Hook 1...............................................(no files to check)Skipped
    Hook 2...............................................(no files to check)Skipped

    ----- stderr -----
    "#);
}
