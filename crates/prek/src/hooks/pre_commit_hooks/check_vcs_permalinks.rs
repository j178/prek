use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use clap::Parser;
use fancy_regex::{Regex, escape};
use tokio::io::{AsyncBufReadExt, BufReader};

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::run::CONCURRENCY;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long = "additional-github-domain")]
    additional_github_domains: Vec<String>,
}

#[derive(Debug)]
struct GithubPermalinkMatcher {
    patterns: Vec<Regex>,
}

impl GithubPermalinkMatcher {
    fn from_hook(hook: &Hook) -> Result<Self> {
        let args =
            Args::try_parse_from(hook.entry.expect_direct().split()?.iter().chain(&hook.args))?;
        Ok(Self::new(args.additional_github_domains))
    }

    fn new(additional_domains: Vec<String>) -> Self {
        let mut domains = BTreeSet::from([String::from("github.com")]);
        domains.extend(additional_domains);

        let patterns = domains
            .into_iter()
            .map(|domain| {
                let domain = escape(&domain);
                let pattern = format!(
                    r"https://{domain}/[^/ ]+/[^/ ]+/blob/(?![a-fA-F0-9]{{4,64}}/)([^/. ]+)/[^# ]+#L\d+"
                );
                Regex::new(&pattern).expect("vcs permalink regex must be valid")
            })
            .collect();

        Self { patterns }
    }

    fn is_non_permalink(&self, line: &[u8]) -> bool {
        let line = String::from_utf8_lossy(line);
        self.patterns
            .iter()
            .any(|pattern| pattern.is_match(&line).unwrap_or(false))
    }
}

pub(crate) async fn check_vcs_permalinks(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let file_base = hook.project().relative_path();
    let matcher = GithubPermalinkMatcher::from_hook(hook)?;

    run_concurrent_file_checks(filenames.iter().copied(), *CONCURRENCY, |filename| {
        check_file(file_base, filename, &matcher)
    })
    .await
}

async fn check_file(
    file_base: &Path,
    filename: &Path,
    matcher: &GithubPermalinkMatcher,
) -> Result<(i32, Vec<u8>)> {
    let path = file_base.join(filename);
    let file = fs_err::tokio::File::open(&path).await?;
    let mut reader = BufReader::new(file);

    let mut retval = 0;
    let mut output = Vec::new();
    let mut line = Vec::new();
    let mut line_number = 0;

    while reader.read_until(b'\n', &mut line).await? != 0 {
        line_number += 1;
        if matcher.is_non_permalink(&line) {
            retval = 1;
            write!(output, "{}:{}:", filename.display(), line_number)?;
            output.write_all(&line)?;
            if !line.ends_with(b"\n") {
                writeln!(output)?;
            }
        }
        line.clear();
    }

    if retval != 0 {
        writeln!(output)?;
        writeln!(output, "Non-permanent github link detected.")?;
        writeln!(
            output,
            "On any page on github press [y] to load a permalink."
        )?;
    }

    Ok((retval, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn matcher(domains: &[&str]) -> GithubPermalinkMatcher {
        GithubPermalinkMatcher::new(domains.iter().map(ToString::to_string).collect())
    }

    #[test]
    fn test_permalink_not_flagged() {
        let matcher = matcher(&[]);
        assert!(
            !matcher
                .is_non_permalink(b"https://github.com/owner/repo/blob/abc123def456/file.py#L10")
        );
        assert!(!matcher.is_non_permalink(
            b"https://github.com/owner/repo/blob/abcdef1234567890abcdef1234567890abcdef12/src/main.rs#L42",
        ));
    }

    #[test]
    fn test_branch_link_flagged() {
        let matcher = matcher(&[]);
        assert!(matcher.is_non_permalink(b"https://github.com/owner/repo/blob/main/file.py#L10"));
        assert!(
            matcher.is_non_permalink(b"https://github.com/owner/repo/blob/master/src/lib.rs#L5")
        );
        assert!(
            matcher.is_non_permalink(b"https://github.com/owner/repo/blob/develop/README.md#L1")
        );
    }

    #[test]
    fn test_no_line_number_not_flagged() {
        let matcher = matcher(&[]);
        assert!(!matcher.is_non_permalink(b"https://github.com/owner/repo/blob/main/file.py"));
    }

    #[test]
    fn test_additional_github_domain_flagged() {
        let matcher = matcher(&["github.example.com"]);
        assert!(
            matcher
                .is_non_permalink(b"https://github.example.com/owner/repo/blob/main/file.py#L10",)
        );
    }

    #[test]
    fn test_github_domains_are_deduplicated() {
        let matcher = GithubPermalinkMatcher::new(vec![
            "github.example.com".to_string(),
            "github.com".to_string(),
            "github.example.com".to_string(),
        ]);
        assert_eq!(matcher.patterns.len(), 2);
    }

    #[tokio::test]
    async fn test_check_file_with_additional_domain() -> Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("links.md");
        fs_err::tokio::write(
            &file_path,
            b"https://github.example.com/owner/repo/blob/main/file.py#L10\n",
        )
        .await?;

        let matcher = matcher(&["github.example.com"]);
        let relative = PathBuf::from("links.md");
        let (code, output) = check_file(dir.path(), &relative, &matcher).await?;

        assert_eq!(code, 1);
        assert_eq!(
            String::from_utf8(output)?,
            "links.md:1:https://github.example.com/owner/repo/blob/main/file.py#L10\n\nNon-permanent github link detected.\nOn any page on github press [y] to load a permalink.\n",
        );

        Ok(())
    }
}
