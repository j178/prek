#[cfg(unix)]
use crate::common::{TestContext, cmd_snapshot};

// TODO: fix this on Windows
//   require resolve `bash.exe` before running scripts.
#[cfg(unix)]
#[test]
fn script_run() {
    let context = TestContext::new();
    context.init_project();
    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: https://github.com/prek-test-repos/script-hooks
            rev: main
            hooks:
              - id: echo
                verbose: true
    "});
    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r##"
    success: true
    exit_code: 0
    ----- stdout -----
    echo.....................................................................Passed
    - hook id: echo
    - duration: [TIME]
      .pre-commit-config.yaml

    ----- stderr -----
    warning: The following repos have mutable `rev` fields (moving tag / branch):
    https://github.com/prek-test-repos/script-hooks: main
    Mutable references are never updated after first install and are not supported.
    See https://pre-commit.com/#using-the-latest-version-for-a-repository for more details.
    Hint: `prek autoupdate` often fixes this",
    "##);
}
