use assert_fs::fixture::{FileWriteStr, PathChild};

use crate::common::{TestContext, cmd_snapshot};

#[cfg(unix)]
#[test]
fn bash_shell_adapter_runs_entry() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: bash-shell
                name: bash-shell
                language: system
                files: ^input\.txt$
                shell: bash
                entry: |
                  items=("$@")
                  printf 'bash:%s:%s\n' "${items[0]}" "${items[1]}"
                args: [configured]
                verbose: true
    "#});
    context.work_dir().child("input.txt").write_str("input")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    bash-shell...............................................................Passed
    - hook id: bash-shell
    - duration: [TIME]

      bash:configured:input.txt

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn pwsh_shell_adapter_runs_entry() -> anyhow::Result<()> {
    if which::which("pwsh").is_err() {
        return Ok(());
    }

    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: pwsh-shell
                name: pwsh-shell
                language: system
                files: ^input\.txt$
                shell: pwsh
                entry: |
                  Write-Output "pwsh:$($args[0]):$($args[1])"
                args: [configured]
                verbose: true
    "#});
    context.work_dir().child("input.txt").write_str("input")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    pwsh-shell...............................................................Passed
    - hook id: pwsh-shell
    - duration: [TIME]

      pwsh:configured:input.txt

    ----- stderr -----
    ");

    Ok(())
}

#[cfg(windows)]
#[test]
fn powershell_shell_adapter_runs_entry() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: powershell-shell
                name: powershell-shell
                language: system
                files: ^input\.txt$
                shell: powershell
                entry: |
                  Write-Output "powershell:$($args[0]):$($args[1])"
                args: [configured]
                verbose: true
    "#});
    context.work_dir().child("input.txt").write_str("input")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    powershell-shell.........................................................Passed
    - hook id: powershell-shell
    - duration: [TIME]

      powershell:configured:input.txt

    ----- stderr -----
    ");

    Ok(())
}

#[cfg(windows)]
#[test]
fn cmd_shell_adapter_runs_entry() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: cmd-shell
                name: cmd-shell
                language: system
                files: ^input\.txt$
                shell: cmd
                entry: |
                  @echo off
                  echo cmd:%1:%2
                args: [configured]
                verbose: true
    "});
    context.work_dir().child("input.txt").write_str("input")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    cmd-shell................................................................Passed
    - hook id: cmd-shell
    - duration: [TIME]

      cmd:configured:input.txt

    ----- stderr -----
    ");

    Ok(())
}

#[test]
fn shell_rejected_for_pygrep() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
              - id: check-todo
                name: check-todo
                language: pygrep
                entry: TODO
                shell: sh
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
      caused by: Invalid hook `check-todo`
      caused by: Hook specified `shell` but the language `pygrep` does not support shell execution: `entry` is the regex pattern
    ");
}
