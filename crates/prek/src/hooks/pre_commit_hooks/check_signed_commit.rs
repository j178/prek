use std::fmt::Write as _;

use anyhow::Result;
use clap::Parser;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use crate::git;
use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::parse_hook_args;

/// Git's per-commit signature status, as reported by `%G?` in `git log`/`git show`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SignatureStatus {
    Good,
    Bad,
    GoodUnknownValidity,
    ExpiredGood,
    ExpiredKey,
    RevokedKey,
    CannotCheck,
    NoSignature,
}

impl SignatureStatus {
    fn from_code(code: &str) -> Option<Self> {
        match code {
            "G" => Some(Self::Good),
            "B" => Some(Self::Bad),
            "U" => Some(Self::GoodUnknownValidity),
            "X" => Some(Self::ExpiredGood),
            "Y" => Some(Self::ExpiredKey),
            "R" => Some(Self::RevokedKey),
            "E" => Some(Self::CannotCheck),
            "N" => Some(Self::NoSignature),
            _ => None,
        }
    }

    fn code(self) -> &'static str {
        match self {
            Self::Good => "G",
            Self::Bad => "B",
            Self::GoodUnknownValidity => "U",
            Self::ExpiredGood => "X",
            Self::ExpiredKey => "Y",
            Self::RevokedKey => "R",
            Self::CannotCheck => "E",
            Self::NoSignature => "N",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Good => "good signature",
            Self::Bad => "bad signature",
            Self::GoodUnknownValidity => "good signature, unknown validity (untrusted)",
            Self::ExpiredGood => "good signature, but expired",
            Self::ExpiredKey => "good signature, made with an expired key",
            Self::RevokedKey => "good signature, made with a revoked key",
            Self::CannotCheck => "signature cannot be checked, e.g. missing public key",
            Self::NoSignature => "no signature",
        }
    }
}

fn parse_allow_status(value: &str) -> Result<SignatureStatus, String> {
    SignatureStatus::from_code(&value.to_ascii_uppercase()).ok_or_else(|| {
        format!("invalid status code `{value}`, expected one of: G, B, U, X, Y, R, E, N")
    })
}

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Signature status code to accept (repeatable): G, B, U, X, Y, R, E, or N.
    #[arg(
        long = "allow-status",
        value_name = "CODE",
        value_parser = parse_allow_status,
        default_values = &["G", "U"]
    )]
    allow_status: Vec<SignatureStatus>,
}

/// Runs the `check-signed-commit` hook.
pub(crate) async fn run(hook: &Hook) -> Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;

    let Some(range) = resolve_range().await? else {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    };

    let stdout = git::git_cmd()?
        .current_dir(hook.work_dir())
        .arg("log")
        .arg("--no-merges")
        .arg("-z")
        .arg("--pretty=format:%h\u{1f}%G?\u{1f}%s")
        .arg(range)
        .check(true)
        .output()
        .await?
        .stdout;

    let offending: Vec<SignedCommit> = parse_signed_commits(&stdout)
        .into_iter()
        .filter(|commit| !args.allow_status.contains(&commit.status))
        .collect();

    if offending.is_empty() {
        Ok(HookOutput::unchanged(0, Vec::new()))
    } else {
        Ok(HookOutput::unchanged(
            1,
            render_message(&offending).into_bytes(),
        ))
    }
}

/// Resolve the commit range to check.
///
/// Uses the push range from `PRE_COMMIT_FROM_REF`/`PRE_COMMIT_TO_REF` when available (the
/// normal `pre-push` invocation). Otherwise falls back to just `HEAD`, so the hook still does
/// something useful when run manually (`prek run check-signed-commit --hook-stage manual`).
async fn resolve_range() -> Result<Option<String>> {
    let from_ref = EnvVars.var("PRE_COMMIT_FROM_REF").ok();
    let to_ref = EnvVars.var("PRE_COMMIT_TO_REF").ok();

    if let Some(to_ref) = to_ref {
        return Ok(Some(match from_ref {
            Some(from_ref) => format!("{from_ref}..{to_ref}"),
            // Root/orphan push: there's no base to diff from, so walk the whole
            // history reachable from `to_ref`.
            None => to_ref,
        }));
    }

    if !git::rev_exists("HEAD").await? {
        // Unborn HEAD: no commits to check yet.
        return Ok(None);
    }

    Ok(Some(match git::parent_commit("HEAD").await? {
        Some(parent) => format!("{parent}..HEAD"),
        // Root commit: there's no parent to diff against, so check HEAD alone.
        None => "HEAD".to_string(),
    }))
}

struct SignedCommit {
    hash: String,
    status: SignatureStatus,
    subject: String,
}

/// Parse NUL-separated `<hash>\x1f<status-code>\x1f<subject>` records from `git log`.
fn parse_signed_commits(stdout: &[u8]) -> Vec<SignedCommit> {
    stdout
        .split(|&b| b == b'\0')
        .filter(|record| !record.is_empty())
        .filter_map(|record| {
            let record = String::from_utf8_lossy(record);
            let mut fields = record.splitn(3, '\u{1f}');
            let hash = fields.next()?;
            let code = fields.next()?;
            let subject = fields.next()?;
            Some(SignedCommit {
                hash: hash.to_string(),
                status: SignatureStatus::from_code(code)?,
                subject: subject.to_string(),
            })
        })
        .collect()
}

fn render_message(offending: &[SignedCommit]) -> String {
    let mut message = String::new();
    for commit in offending {
        writeln!(
            message,
            "{} [{}] {}: {}",
            commit.hash,
            commit.status.code(),
            commit.status.description(),
            commit.subject
        )
        .expect("writing to String should never fail");
    }

    message.push('\n');
    message.push_str(indoc::indoc! {"
        Commit signature status codes:
          G  good signature
          B  bad signature
          U  good signature, unknown validity (untrusted)
          X  good signature, but expired
          Y  good signature, made with an expired key
          R  good signature, made with a revoked key
          E  signature cannot be checked, e.g. missing public key
          N  no signature

        Pass `--allow-status <CODE>` (repeatable) to accept additional codes.
    "});

    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signature_status_round_trips_known_codes() {
        for code in ["G", "B", "U", "X", "Y", "R", "E", "N"] {
            let status = SignatureStatus::from_code(code).unwrap();
            assert_eq!(status.code(), code);
        }
    }

    #[test]
    fn signature_status_rejects_unknown_code() {
        assert!(SignatureStatus::from_code("?").is_none());
        assert!(SignatureStatus::from_code("").is_none());
        assert!(SignatureStatus::from_code("g").is_none());
    }

    #[test]
    fn parse_allow_status_accepts_lowercase() {
        assert_eq!(parse_allow_status("g"), Ok(SignatureStatus::Good));
    }

    #[test]
    fn parse_allow_status_rejects_unknown_code() {
        let err = parse_allow_status("Q").unwrap_err();
        assert_eq!(
            err,
            "invalid status code `Q`, expected one of: G, B, U, X, Y, R, E, N"
        );
    }

    #[test]
    fn args_default_to_good_and_untrusted() {
        let args = Args::try_parse_from(["check-signed-commit"]).unwrap();
        assert_eq!(
            args.allow_status,
            vec![SignatureStatus::Good, SignatureStatus::GoodUnknownValidity]
        );
    }

    #[test]
    fn args_allow_status_is_repeatable_and_overrides_defaults() {
        let args = Args::try_parse_from([
            "check-signed-commit",
            "--allow-status",
            "N",
            "--allow-status",
            "E",
        ])
        .unwrap();
        assert_eq!(
            args.allow_status,
            vec![SignatureStatus::NoSignature, SignatureStatus::CannotCheck]
        );
    }

    #[test]
    fn parse_signed_commits_reads_multiple_nul_separated_records() {
        let stdout = b"abc123\x1fG\x1ffirst commit\0def456\x1fN\x1fsecond commit\0";

        let commits = parse_signed_commits(stdout);

        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "abc123");
        assert_eq!(commits[0].status, SignatureStatus::Good);
        assert_eq!(commits[0].subject, "first commit");
        assert_eq!(commits[1].hash, "def456");
        assert_eq!(commits[1].status, SignatureStatus::NoSignature);
        assert_eq!(commits[1].subject, "second commit");
    }

    #[test]
    fn parse_signed_commits_ignores_malformed_records() {
        let stdout = b"onlyhash\0abc123\x1fZ\x1funknown status code\0def456\x1fG\x1fgood\0";

        let commits = parse_signed_commits(stdout);

        assert_eq!(commits.len(), 1);
        assert_eq!(commits[0].hash, "def456");
    }

    #[test]
    fn render_message_lists_each_offending_commit_and_a_legend() {
        let offending = vec![SignedCommit {
            hash: "abc123".to_string(),
            status: SignatureStatus::NoSignature,
            subject: "oops".to_string(),
        }];

        let message = render_message(&offending);

        assert!(message.contains("abc123 [N] no signature: oops"));
        assert!(message.contains("Commit signature status codes:"));
        assert!(message.contains("--allow-status"));
    }
}
