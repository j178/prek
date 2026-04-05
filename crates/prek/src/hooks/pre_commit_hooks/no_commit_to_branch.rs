use std::sync::LazyLock;

use clap::Parser;
use fancy_regex::Regex;

use crate::git::{GIT, git_cmd};
use crate::hook::Hook;
use anyhow::{Context, Result};

/// Build the default protected branch list from a base set plus an optional detected branch.
fn default_branches_with(detected: Option<&str>) -> Vec<String> {
    let mut branches = vec!["main".to_string(), "master".to_string()];
    if let Some(branch) = detected {
        let trimmed = branch.trim().trim_start_matches("origin/");
        if !trimmed.is_empty() && !branches.iter().any(|b| b == trimmed) {
            branches.push(trimmed.to_string());
        }
    }
    branches
}

/// Default protected branches: "main", "master", and the repo's default branch pointed to
/// by `origin/HEAD` (if detectable and not already covered by main+master)
static DEFAULT_BRANCHES: LazyLock<Vec<String>> = LazyLock::new(|| {
    let detected = GIT.as_ref().ok().and_then(|git| {
        let output = std::process::Command::new(git)
            .arg("symbolic-ref")
            .arg("--short")
            .arg("origin/HEAD")
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    });
    default_branches_with(detected.as_deref())
});

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(short, long = "branch", default_values_t = DEFAULT_BRANCHES.clone())]
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
    let args = Args::try_parse_from(hook.entry.split()?.iter().chain(&hook.args))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Args {
        Args::try_parse_from(std::iter::once("no-commit-to-branch").chain(args.iter().copied()))
            .unwrap()
    }

    #[test]
    fn defaults_include_main_and_master() {
        let args = parse_args(&[]);
        assert!(args.branches.contains(&"main".to_string()));
        assert!(args.branches.contains(&"master".to_string()));
    }

    #[test]
    fn explicit_branches_override_defaults() {
        let args = parse_args(&["-b", "develop", "-b", "release"]);
        assert_eq!(args.branches, vec!["develop", "release"]);
        assert!(!args.branches.contains(&"main".to_string()));
    }

    #[test]
    fn check_protected_matches_exact_branch() {
        let args = parse_args(&["-b", "main"]);
        assert!(args.check_protected("main").unwrap());
        assert!(!args.check_protected("main-feature").unwrap());
    }

    #[test]
    fn check_protected_no_match_returns_false() {
        let args = parse_args(&["-b", "main"]);
        assert!(!args.check_protected("develop").unwrap());
    }

    #[test]
    fn check_protected_with_pattern() {
        let args = parse_args(&["-b", "main", "-p", "^release/.*$"]);
        assert!(args.check_protected("release/1.0").unwrap());
        assert!(!args.check_protected("feature/release/1.0").unwrap());
    }

    #[test]
    fn check_protected_pattern_without_branch_match() {
        let args = parse_args(&["-b", "nope", "-p", "^hotfix/"]);
        assert!(!args.check_protected("main").unwrap());
        assert!(args.check_protected("hotfix/urgent").unwrap());
    }

    #[test]
    fn check_protected_no_patterns_ignores_regex() {
        let args = parse_args(&["-b", "main"]);
        // No patterns set, so only exact branch match applies
        assert!(!args.check_protected("anything-else").unwrap());
    }

    #[test]
    fn invalid_regex_pattern_returns_error() {
        let args = parse_args(&["-b", "main", "-p", "([invalid"]);
        assert!(args.check_protected("some-branch").is_err());
    }

    #[test]
    fn default_branches_includes_detected_non_standard_branch() {
        let branches = default_branches_with(Some("origin/develop"));
        assert!(branches.contains(&"main".to_string()));
        assert!(branches.contains(&"master".to_string()));
        assert!(branches.contains(&"develop".to_string()));
    }

    #[test]
    fn default_branches_does_not_duplicate_main() {
        let branches = default_branches_with(Some("origin/main"));
        assert_eq!(branches.iter().filter(|b| *b == "main").count(), 1);
    }

    #[test]
    fn default_branches_does_not_duplicate_master() {
        let branches = default_branches_with(Some("origin/master"));
        assert_eq!(branches.iter().filter(|b| *b == "master").count(), 1);
    }

    #[test]
    fn default_branches_handles_none_detected() {
        let branches = default_branches_with(None);
        assert_eq!(branches, vec!["main", "master"]);
    }

    #[test]
    fn default_branches_ignores_empty_detected() {
        let branches = default_branches_with(Some(""));
        assert_eq!(branches, vec!["main", "master"]);
    }

    #[test]
    fn default_branches_strips_origin_prefix() {
        let branches = default_branches_with(Some("origin/production"));
        assert!(branches.contains(&"production".to_string()));
        assert!(!branches.iter().any(|b| b.contains("origin/")));
    }
}
