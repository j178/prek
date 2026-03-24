use std::borrow::Cow;
use std::path::Path;

use anyhow::Context;
use itertools::Itertools;
use prek_consts::env_vars::EnvVars;

use crate::git;
use crate::hook::Hook;

pub(crate) async fn forbid_new_submodules(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>), anyhow::Error> {
    let diff_arg = if let (Ok(from_ref), Ok(to_ref)) = (
        EnvVars::var("PRE_COMMIT_FROM_REF"),
        EnvVars::var("PRE_COMMIT_TO_REF"),
    ) {
        Cow::Owned(format!("{from_ref}...{to_ref}"))
    } else {
        Cow::Borrowed("--staged")
    };

    let stdout = git::git_cmd("git diff")?
        .current_dir(hook.work_dir())
        .arg("diff")
        .arg("--diff-filter=A")
        .arg("--raw")
        .arg(diff_arg.as_ref())
        .arg("--")
        .args(filenames)
        .check(true)
        .output()
        .await?
        .stdout;

    let new_submodules = collect_new_submodules(&stdout)?;
    let Some(message) = render_message(&new_submodules) else {
        return Ok((0, Vec::new()));
    };

    Ok((1, message.into_bytes()))
}

fn render_message(new_submodules: &[&str]) -> Option<String> {
    if new_submodules.is_empty() {
        return None;
    }

    let mut message = new_submodules
        .iter()
        .map(|filename| format!("{filename}: new submodule introduced"))
        .join("\n");

    message.push_str("\n\n");
    message.push_str(indoc::indoc! {"
        This commit introduces new git submodules.
        Did you unintentionally `git add .`?
        To fix this, run `git rm <submodule>`.
        Also check `.gitmodules` for any unintended changes.
    "});

    Some(message)
}

fn collect_new_submodules(stdout: &[u8]) -> Result<Vec<&str>, anyhow::Error> {
    let mut new_submodules = Vec::new();
    for line in std::str::from_utf8(stdout)?.lines() {
        let (metadata, filename) = line
            .split_once('\t')
            .context("couldn't parse raw diff output")?;
        let file_mode = metadata
            .split_whitespace()
            .nth(1)
            .context("couldn't get file-mode from raw diff output")?;

        if file_mode == "160000" {
            new_submodules.push(filename);
        }
    }
    Ok(new_submodules)
}

#[cfg(test)]
mod tests {
    #[test]
    fn collect_new_submodules_ignores_non_submodules() {
        let stdout = b":000000 100644 0000000 abcdef1 A\t.gitmodules\n";

        let new_submodules =
            super::collect_new_submodules(stdout).expect("diff output should parse");

        assert!(new_submodules.is_empty());
    }

    #[test]
    fn collect_new_submodules_finds_new_submodules() {
        let stdout = indoc::indoc! {"
            :000000 100644 0000000 abcdef1 A\t.gitmodules
            :000000 160000 0000000 abcdef2 A\tproject2/sub module
            :000000 160000 0000000 abcdef3 A\tvendor/dep
        "}
        .as_bytes();

        let new_submodules =
            super::collect_new_submodules(stdout).expect("diff output should parse");

        assert_eq!(new_submodules, vec!["project2/sub module", "vendor/dep"]);
    }

    #[test]
    fn render_message() {
        let message = super::render_message(&["project2/sub module", "vendor/dep"])
            .expect("submodules should be reported");

        assert_eq!(
            message,
            indoc::indoc! {"
                project2/sub module: new submodule introduced
                vendor/dep: new submodule introduced

                This commit introduces new git submodules.
                Did you unintentionally `git add .`?
                To fix this, run `git rm <submodule>`.
                Also check `.gitmodules` for any unintended changes.
            "}
        );
    }
}
