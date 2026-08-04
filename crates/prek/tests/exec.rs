mod common;

use anyhow::Result;
use indoc::indoc;

#[cfg(unix)]
use crate::common::make_executable;
use crate::common::{TestContext, cmd_snapshot};

fn config() -> &'static str {
    indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: exec-test
                name: Exec test
                entry: command-that-must-not-run
                language: system
                args: [argument-that-must-not-be-passed]
                env:
                  GIT_AUTHOR_NAME: Prek Exec
                  GIT_AUTHOR_EMAIL: exec@prek.dev
                  GIT_AUTHOR_DATE: "2000-01-01T00:00:00+00:00"
    "#}
}

fn context_with_config() -> TestContext {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(config());
    context
}

#[test]
fn exec_inherits_stdin_and_stdout() {
    let context = context_with_config();

    cmd_snapshot!(context.filters(), context.exec()
        .args(["exec-test", "--", "git", "stripspace"])
        .pass_stdin("hello from prek exec\n"), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    hello from prek exec

    ----- stderr -----
    ");
}

#[test]
fn exec_applies_hook_environment_without_entry_or_args() {
    let context = context_with_config();

    cmd_snapshot!(context.filters(), context.exec().args([
            "exec-test",
            "--",
            "git",
            "var",
            "GIT_AUTHOR_IDENT",
        ]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    Prek Exec <exec@prek.dev> 946684800 +0000

    ----- stderr -----
    ");
}

#[test]
fn exec_explicit_selector_ignores_skip_environment() {
    let context = context_with_config();

    cmd_snapshot!(context.filters(), context.exec()
        .env("PREK_SKIP", "exec-test")
        .args([
            "exec-test",
            "--",
            "git",
            "rev-parse",
            "--is-inside-work-tree",
        ]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    true

    ----- stderr -----
    ");
}

#[test]
fn exec_propagates_child_exit_status() {
    let context = context_with_config();

    cmd_snapshot!(context.filters(), context.exec().args([
            "exec-test",
            "--",
            "git",
            "-c",
            "alias.exit-42=!exit 42",
            "exit-42",
        ]), @r"
    success: false
    exit_code: 42
    ----- stdout -----

    ----- stderr -----
    ");
}

#[test]
fn exec_rejects_ambiguous_hook_selector() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.setup_workspace(&["frontend"], config())?;

    cmd_snapshot!(context.filters(), context.exec().args([
        "exec-test",
        "--",
        "git",
        "rev-parse",
        "--is-inside-work-tree",
    ]), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: Hook selector `exec-test` matched multiple hooks:
      - frontend:exec-test
      - .:exec-test
    Use a `project-path:hook-id` selector to select one hook
    ");
    Ok(())
}

#[test]
fn exec_keeps_current_working_directory() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.setup_workspace(&["frontend"], config())?;

    cmd_snapshot!(context.filters(), context.exec().args([
            "frontend:exec-test",
            "--",
            "git",
            "rev-parse",
            "--show-prefix",
        ]), @r"
    success: true
    exit_code: 0
    ----- stdout -----


    ----- stderr -----
    ");
    Ok(())
}

#[cfg(unix)]
#[test]
fn exec_resolves_relative_command_from_current_working_directory() -> Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.setup_workspace(&["frontend"], config())?;

    let command = context.work_dir().join("exec-tool");
    fs_err::write(&command, "#!/bin/sh\necho relative command ok\n")?;
    make_executable(&command)?;

    cmd_snapshot!(context.filters(), context.exec().args([
        "frontend:exec-test",
        "--",
        "./exec-tool",
    ]), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    relative command ok

    ----- stderr -----
    ");
    Ok(())
}

#[test]
fn exec_rejects_unsupported_language_before_install() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: julia-hook
                name: Julia hook
                entry: hook.jl
                language: julia
    "});

    cmd_snapshot!(context.filters(), context.exec().args([
        "julia-hook",
        "--",
        "git",
        "rev-parse",
        "--is-inside-work-tree",
    ]), @r"
    success: false
    exit_code: 2
    ----- stdout -----

    ----- stderr -----
    error: `prek exec` does not support hooks with language `julia`
    ");
}
