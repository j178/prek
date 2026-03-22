use std::path::Path;

use anyhow::Context;

use crate::git;
use crate::hook::Hook;

pub(crate) async fn forbid_new_submodules(
    _hook: &Hook,
    _filenames: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let stdout = git::git_cmd("check staged items for submodule addition")?
        .arg("diff")
        .arg("--diff-filter=A")
        .arg("--raw")
        .arg("--staged")
        .check(true)
        .output()
        .await?
        .stdout;

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
                To fix: git rm {{thesubmodule}}  # no trailing slash\n\
                Also check .gitmodules"
            );

            return Ok((1, msg.into_bytes()));
        }
    }

    Ok((0, Vec::new()))
}
