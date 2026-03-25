use std::borrow::Cow;
use std::path::Path;

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
        .arg("--relative")
        .arg("--diff-filter=A")
        .arg("--no-ext-diff")
        .arg("--raw")
        .arg("-z")
        .arg(diff_arg.as_ref())
        .arg("--")
        .args(filenames)
        .check(true)
        .output()
        .await?
        .stdout;

    let new_submodules = collect_new_submodules(&stdout);
    if new_submodules.is_empty() {
        Ok((0, Vec::new()))
    } else {
        let message = render_message(&new_submodules);
        Ok((1, message.into_bytes()))
    }
}

fn render_message(new_submodules: &[&str]) -> String {
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

    message
}

fn collect_new_submodules(stdout: &[u8]) -> Vec<&str> {
    let mut entries = stdout
        .split(|&b| b == b'\0')
        .filter(|entry| !entry.is_empty());
    let mut new_submodules = Vec::new();

    while let Some(metadata) = entries.next() {
        let Some(filename) = entries.next() else {
            break;
        };

        let Ok(metadata) = std::str::from_utf8(metadata) else {
            continue;
        };
        let Ok(filename) = std::str::from_utf8(filename) else {
            continue;
        };

        // https://git-scm.com/docs/gitdatamodel#Documentation/gitdatamodel.txt-tree
        if metadata.split_whitespace().nth(1) == Some("160000") {
            new_submodules.push(filename);
        }
    }

    new_submodules
}

#[cfg(test)]
mod tests {
    #[test]
    fn collect_new_submodules_ignores_non_submodules() {
        let stdout = b":000000 100644 0000000 abcdef1 A\0.gitmodules\0";

        let new_submodules = super::collect_new_submodules(stdout);

        assert!(new_submodules.is_empty());
    }

    #[test]
    fn collect_new_submodules_finds_new_submodules() {
        let stdout = b":000000 100644 0000000 abcdef1 A\0.gitmodules\0\
:000000 160000 0000000 abcdef2 A\0project2/sub module\0\
:000000 160000 0000000 abcdef3 A\0vendor/dep\0";

        let new_submodules = super::collect_new_submodules(stdout);

        assert_eq!(new_submodules, vec!["project2/sub module", "vendor/dep"]);
    }

    #[test]
    fn render_message() {
        let message = super::render_message(&["project2/sub module", "vendor/dep"]);

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
