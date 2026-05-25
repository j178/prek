#[cfg(unix)]
use crate::common::{TestContext, cmd_snapshot};
#[cfg(unix)]
use assert_fs::fixture::{FileWriteStr, PathChild};

#[cfg(unix)]
#[test]
fn multiline_entry_without_shell_uses_argv_semantics() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
    repos:
      - repo: local
        hooks:
          - id: no-shell
            name: no-shell
            language: system
            entry: |
              echo first
              echo second
            pass_filenames: false
            verbose: true
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    no-shell.................................................................Passed
    - hook id: no-shell
    - duration: [TIME]

      first echo second

    ----- stderr -----
    ");
}

#[cfg(unix)]
#[test]
fn shell_runs_multiline_entry_as_one_script() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
    repos:
      - repo: local
        hooks:
          - id: shell-script
            name: shell-script
            language: system
            entry: |
              echo first
              echo second
            shell: sh
            pass_filenames: false
            verbose: true
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    shell-script.............................................................Passed
    - hook id: shell-script
    - duration: [TIME]

      first
      second

    ----- stderr -----
    ");
}

#[cfg(unix)]
#[test]
fn shell_entry_receives_hook_args_before_filenames() -> anyhow::Result<()> {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
    repos:
      - repo: local
        hooks:
          - id: shell-args
            name: shell-args
            language: system
            files: ^a\.txt$
            entry: |
              printf 'args:'
              for value in "$@"; do
                printf ' <%s>' "$value"
              done
              printf '\n'
            shell: sh
            args: [configured]
            verbose: true
    "#});
    context.work_dir().child("a.txt").write_str("a")?;
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r"
    success: true
    exit_code: 0
    ----- stdout -----
    shell-args...............................................................Passed
    - hook id: shell-args
    - duration: [TIME]

      args: <configured> <a.txt>

    ----- stderr -----
    ");

    Ok(())
}
