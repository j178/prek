use std::collections::BTreeSet;
use std::io::Write;
use std::path::Path;

use anyhow::Result;
use clap::Parser;
use memchr::memmem;
use regex::bytes::{Match, Regex};
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
struct GithubNonPermalinkMatcher {
    checks: Vec<GithubNonPermalinkCheck>,
}

#[derive(Debug)]
struct GithubNonPermalinkCheck {
    needle: Vec<u8>,
    pattern: Regex,
}

impl GithubNonPermalinkMatcher {
    fn new(additional_domains: Vec<String>) -> Self {
        let mut domains = BTreeSet::from([String::from("github.com")]);
        domains.extend(additional_domains);

        let checks = domains
            .into_iter()
            .map(|domain| {
                let needle = format!("https://{domain}/").into_bytes();
                let domain = regex::escape(&domain);
                let pattern = format!(r"https://{domain}/[^/ ]+/[^/ ]+/blob/([^/. ]+)/[^# ]+#L\d+");
                GithubNonPermalinkCheck {
                    needle,
                    pattern: Regex::new(&pattern).expect("vcs permalink regex must be valid"),
                }
            })
            .collect();

        Self { checks }
    }

    fn find_non_permalink<'a>(&self, line: &'a [u8]) -> impl Iterator<Item = Match<'a>> {
        let mut matches = self
            .checks
            .iter()
            .filter(|check| memmem::find(line, &check.needle).is_some())
            .flat_map(|check| {
                check.pattern.captures_iter(line).filter_map(|captures| {
                    let reference = captures.get(1)?;
                    if is_probable_commit_hash(reference.as_bytes()) {
                        None
                    } else {
                        captures.get(0)
                    }
                })
            })
            .collect::<Vec<_>>();
        matches.sort_unstable_by_key(Match::start);
        matches.into_iter()
    }
}

fn is_probable_commit_hash(reference: &[u8]) -> bool {
    (4..=64).contains(&reference.len()) && reference.iter().all(u8::is_ascii_hexdigit)
}

pub(crate) async fn check_vcs_permalinks(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.expect_direct().split()?.iter().chain(&hook.args))?;
    let matcher = GithubNonPermalinkMatcher::new(args.additional_github_domains);

    let file_base = hook.project().relative_path();
    run_concurrent_file_checks(filenames.iter().copied(), *CONCURRENCY, |filename| {
        check_file(file_base, filename, &matcher)
    })
    .await
}

async fn check_file(
    file_base: &Path,
    filename: &Path,
    matcher: &GithubNonPermalinkMatcher,
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
        for m in matcher.find_non_permalink(&line) {
            retval = 1;
            write!(output, "Non-permanent github link detected: ")?;
            write!(output, "{}:{}:", filename.display(), line_number)?;
            output.write_all(m.as_bytes())?;
            writeln!(output)?;
        }
        line.clear();
    }

    Ok((retval, output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn matcher(domains: &[&str]) -> GithubNonPermalinkMatcher {
        GithubNonPermalinkMatcher::new(domains.iter().map(ToString::to_string).collect())
    }

    #[test]
    fn test_permalink_not_flagged() {
        let matcher = matcher(&[]);
        assert!(
            matcher
                .find_non_permalink(b"https://github.com/owner/repo/blob/abc123def456/file.py#L10")
                .next()
                .is_none()
        );
        assert!(
            matcher
                .find_non_permalink(
                    b"https://github.com/owner/repo/blob/abcdef1234567890abcdef1234567890abcdef12/src/main.rs#L42",
                )
                .next()
                .is_none()
        );
    }

    #[test]
    fn test_branch_link_flagged() {
        let matcher = matcher(&[]);
        assert!(
            matcher
                .find_non_permalink(b"https://github.com/owner/repo/blob/main/file.py#L10")
                .next()
                .is_some()
        );
        assert!(
            matcher
                .find_non_permalink(b"https://github.com/owner/repo/blob/master/src/lib.rs#L5")
                .next()
                .is_some()
        );
        assert!(
            matcher
                .find_non_permalink(b"https://github.com/owner/repo/blob/develop/README.md#L1")
                .next()
                .is_some()
        );
    }

    #[test]
    fn test_no_line_number_not_flagged() {
        let matcher = matcher(&[]);
        assert!(
            matcher
                .find_non_permalink(b"https://github.com/owner/repo/blob/main/file.py")
                .next()
                .is_none()
        );
    }

    #[test]
    fn test_additional_github_domain_flagged() {
        let matcher = matcher(&["github.example.com"]);
        assert!(
            matcher
                .find_non_permalink(b"https://github.example.com/owner/repo/blob/main/file.py#L10",)
                .next()
                .is_some()
        );
    }

    #[test]
    fn test_find_non_permalink_returns_all_url_matches_in_order() {
        let matcher = matcher(&["github.example.com"]);
        let line = b"See https://github.example.com/owner/repo/blob/main/file.py#L10 and https://github.com/owner/repo/blob/master/src/lib.rs#L5";

        let urls = matcher
            .find_non_permalink(line)
            .map(|m| m.as_bytes())
            .collect::<Vec<_>>();

        assert_eq!(
            urls,
            vec![
                b"https://github.example.com/owner/repo/blob/main/file.py#L10".as_slice(),
                b"https://github.com/owner/repo/blob/master/src/lib.rs#L5".as_slice(),
            ],
        );
    }

    #[tokio::test]
    async fn test_check_file_with_additional_domain() -> Result<()> {
        let dir = tempdir()?;
        let file_path = dir.path().join("links.md");
        fs_err::tokio::write(
            &file_path,
            b"https://github.example.com/owner/repo/blob/main/file.py#L10 and https://github.com/owner/repo/blob/master/src/lib.rs#L5\n",
        )
        .await?;

        let matcher = matcher(&["github.example.com"]);
        let relative = PathBuf::from("links.md");
        let (code, output) = check_file(dir.path(), &relative, &matcher).await?;

        assert_eq!(code, 1);
        assert_eq!(
            String::from_utf8(output)?,
            "Non-permanent github link detected: links.md:1:https://github.example.com/owner/repo/blob/main/file.py#L10\nNon-permanent github link detected: links.md:1:https://github.com/owner/repo/blob/master/src/lib.rs#L5\n",
        );

        Ok(())
    }
}
