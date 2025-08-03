use crate::common::{TestContext, cmd_snapshot};

/// `pygrep` is not support for now, ensure its weird `entry` does not cause any issues.
#[test]
fn weird_entry() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r#"
        repos:
          - repo: local
            hooks:
              - id: pygrep
                name: pygrep
                language: pygrep
                entry: "default_args\\s*=\\s*{\\s*(\"|')start_date(\"|')|(\"|')start_date(\"|'):"
                always_run: true
                pass_filenames: true
        "#});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: true
    exit_code: 0
    ----- stdout -----
    pygrep...............................................(unimplemented yet)Skipped

    ----- stderr -----
    "#);
}
