use std::env;
use std::path::Path;

use anyhow::Context;

use crate::git;
use crate::hook::Hook;

pub(crate) async fn forbid_new_submodules(
    _hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let mut cmd = git::git_cmd("check staged items")?;
    cmd.arg("diff");

    if let (Ok(from_ref), Ok(to_ref)) = (
        env::var("PRE_COMMIT_FROM_REF"),
        env::var("PRE_COMMIT_TO_REF"),
    ) {
        cmd.arg(format!("{}...{}", from_ref, to_ref));
    } else {
        cmd.arg("--staged");
    }

    cmd.arg("--diff-filter=A").arg("--raw").arg("--");

    for path in filenames {
        cmd.arg(path);
    }

    let stdout = cmd.check(true).output().await?.stdout;

    let stdout_str = std::str::from_utf8(&stdout)?;

    for line in stdout_str.lines() {
        let mut parts = line.split_ascii_whitespace();
        let file_mode = parts
            .nth(1)
            .context("couldn't get file-mode from raw diff output")?;
        let file_name = parts
            .nth(3)
            .context("couldn't get file-name from raw diff output")?;

        if file_mode == "160000" {
            let msg = format!(
                "{file_name}: new submodule introduced\n\
                This commit introduces new submodules.\n\
                Did you unintentionally `git add .`?\n\
                To fix: \n\
                    1. git rm {{thesubmodule}}  # no trailing slash\n\
                    2. rm -rf .git/modules/{{thesubmodule}}  # manually remove this item\n\
                Also check .gitmodules"
            );

            return Ok((1, msg.into_bytes()));
        }
    }

    Ok((0, Vec::new()))
}
