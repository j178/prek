use anyhow::Result;
use assert_fs::prelude::*;

use crate::common::{TestContext, cmd_snapshot};

/// GitHub Action only has docker for linux hosted runners.
#[test]
fn fail() -> Result<()> {
    let context = TestContext::new();

    context.init_project();

    let cwd = context.work_dir();
    cwd.child("changelog").create_dir_all()?;
    cwd.child("changelog/changelog.md").touch()?;

    context.write_pre_commit_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
            - id: changelogs-rst
              name: changelogs must be rst
              entry: changelog filenames must end in .rst
              language: fail
              files: 'changelog/.*(?<!\.rst)$'
    "});

    context.git_add(".");

    cmd_snapshot!(context.filters(), context.run(), @r#"
    success: false
    exit_code: 1
    ----- stdout -----
    changelogs must be rst...................................................Failed
    - hook id: changelogs-rst
    - exit code: 1
      changelog filenames must end in .rst

      changelog/changelog.md

    ----- stderr -----
    "#);

    Ok(())
}
