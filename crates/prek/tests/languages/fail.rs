use anyhow::Result;
use assert_fs::prelude::*;

use crate::common::{TestEnv, cmd_snapshot};

/// GitHub Action only has docker for linux hosted runners.
#[test]
fn fail() -> Result<()> {
    let context = TestEnv::new();

    let cwd = context.work_dir();
    cwd.child("changelog").create_dir_all()?;
    cwd.child("changelog/changelog.md").touch()?;

    let context = context.with_config(indoc::indoc! {r"
        repos:
          - repo: local
            hooks:
            - id: changelogs-rst
              name: changelogs must be rst
              entry: changelog filenames must end in .rst
              language: fail
              files: 'changelog/.*(?<!\.rst)$'
    "});

    context.git_add_all();

    cmd_snapshot!(context, context.run(), @r"
    success: false
    exit_code: 1
    ----- stdout -----
    changelogs must be rst...................................................Failed
    - hook id: changelogs-rst
    - exit code: 1

      changelog filenames must end in .rst

      changelog/changelog.md

    ----- stderr -----
    ");

    Ok(())
}
