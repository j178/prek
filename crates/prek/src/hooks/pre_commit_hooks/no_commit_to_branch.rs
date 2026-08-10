use clap::Parser;
use fancy_regex::Regex;

use crate::git::git_cmd;
use crate::hook::Hook;
use crate::hooks::HookOutput;
use anyhow::{Context, Result};

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Branch to protect (repeatable).
    #[arg(
        short,
        long = "branch",
        value_name = "BRANCH",
        default_values = &["main", "master"]
    )]
    branches: Vec<String>,
    /// Regular expression matching branches to protect (repeatable).
    #[arg(short, long = "pattern", value_name = "REGEX")]
    patterns: Vec<String>,
}

impl Args {
    fn check_protected(&self, branch: &str) -> Result<bool> {
        if self.branches.iter().any(|b| b == branch) {
            return Ok(true);
        }

        for pattern in &self.patterns {
            let regex = Regex::new(pattern)
                .with_context(|| format!("Failed to compile regex pattern `{pattern}`"))?;
            if regex.is_match(branch).with_context(|| {
                format!("Failed to match branch against regex pattern `{pattern}`")
            })? {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

/// Runs the `no-commit-to-branch` hook.
pub(crate) async fn run(hook: &Hook) -> Result<HookOutput> {
    let args = Args::try_parse_from(hook.entry.expect_direct().split_with_args(&hook.args)?)?;

    let output = git_cmd()?
        .arg("symbolic-ref")
        .arg("HEAD")
        .check(false)
        .output()
        .await?;

    if !output.status.success() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    let ref_name = String::from_utf8_lossy(&output.stdout);
    // stdout must start with "refs/heads/"
    let branch = ref_name.trim().trim_start_matches("refs/heads/");

    if args.check_protected(branch)? {
        let err_msg = format!("You are not allowed to commit to branch '{branch}'\n");
        Ok(HookOutput::unchanged(1, err_msg.into_bytes()))
    } else {
        Ok(HookOutput::unchanged(0, Vec::new()))
    }
}

#[cfg(test)]
mod tests {
    use super::Args;

    #[test]
    fn pattern_runtime_errors_include_pattern() {
        let args = Args {
            branches: Vec::new(),
            patterns: vec![r"^(a|aa)+\1$".to_string()],
        };
        let branch = format!("{}b", "a".repeat(40));

        let err = args.check_protected(&branch).unwrap_err();
        assert_eq!(
            err.to_string(),
            r"Failed to match branch against regex pattern `^(a|aa)+\1$`"
        );
    }
}
