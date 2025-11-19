use clap::Parser;
use fancy_regex::Regex;

use crate::git::git_cmd;
use crate::hook::Hook;
use anyhow::{Context, Result};

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(short, long = "branch", default_values = &["main", "master"])]
    branches: Vec<String>,
    #[arg(short, long = "pattern")]
    patterns: Vec<String>,
}

impl Args {
    fn check_protected(&self, branch: &str) -> Result<bool> {
        if self.branches.iter().any(|b| b == branch) {
            return Ok(true);
        }

        if self.patterns.is_empty() {
            return Ok(false);
        }

        let patterns = self
            .patterns
            .iter()
            .map(|p| Regex::new(p))
            .collect::<Result<Vec<Regex>, _>>()
            .context("Failed to compile regex patterns")?;

        Ok(patterns
            .iter()
            .any(|pattern| pattern.is_match(branch).unwrap_or(false)))
    }
}

pub(crate) async fn no_commit_to_branch(hook: &Hook) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    let output = git_cmd("get current branch")?
        .arg("symbolic-ref")
        .arg("HEAD")
        .check(false)
        .output()
        .await?;

    if !output.status.success() {
        return Ok((0, Vec::new()));
    }

    let ref_name = String::from_utf8_lossy(&output.stdout);
    // stdout must start with "refs/heads/"
    let branch = ref_name.trim().trim_start_matches("refs/heads/");

    if args.check_protected(branch)? {
        let err_msg = format!("You are not allowed to commit to branch '{branch}'\n");
        Ok((1, err_msg.into_bytes()))
    } else {
        Ok((0, Vec::new()))
    }
}
