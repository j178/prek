use std::io::Write as _;

use anyhow::{Context, Result};
use futures_util::StreamExt;
use prek_consts::env_vars::{EnvVars, EnvVarsRead};

use crate::git;
use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::run::INTERNAL_CONCURRENCY;

/// Runs the `check-signed-commit` hook.
pub(crate) async fn run(hook: &Hook) -> Result<HookOutput> {
    let Some(range) = resolve_range().await? else {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    };

    let output = git::git_cmd()?
        .current_dir(hook.work_dir())
        .args([
            "log",
            "--no-merges",
            "--no-show-signature",
            "-z",
            "--pretty=format:%H %h %s",
        ])
        .arg(range)
        .output()
        .await?;

    let commits = String::from_utf8_lossy(&output.stdout);
    let mut checks =
        futures_util::stream::iter(commits.split('\0').filter(|record| !record.is_empty()))
            .map(|record| async move {
                let (oid, summary) = record
                    .split_once(' ')
                    .context("Invalid commit information from Git")?;
                // Verify separately so each diagnostic belongs to a specific commit.
                let output = git::git_cmd()?
                    .current_dir(hook.work_dir())
                    .args(["verify-commit", oid])
                    .check(false)
                    .output()
                    .await
                    .with_context(|| format!("Failed to verify commit {oid}"))?;
                anyhow::Ok((summary, output))
            })
            .buffered(*INTERNAL_CONCURRENCY);

    let mut failed = false;
    let mut message = output.stderr;
    while let Some(result) = checks.next().await {
        let (summary, output) = result?;
        if output.status.success() {
            continue;
        }

        failed = true;
        writeln!(message, "{summary}: signature verification failed")?;
        for line in String::from_utf8_lossy(&output.stderr).trim_end().lines() {
            writeln!(message, "  {line}")?;
        }
    }

    Ok(HookOutput::unchanged(i32::from(failed), message))
}

/// Resolve the commit range to check.
///
/// Uses the push range from `PRE_COMMIT_FROM_REF`/`PRE_COMMIT_TO_REF` when available (the
/// normal `pre-push` invocation). Otherwise falls back to just `HEAD`, so the hook still does
/// something useful when run manually (`prek run check-signed-commit --hook-stage manual`).
async fn resolve_range() -> Result<Option<String>> {
    let from_ref = EnvVars.var(EnvVars::PRE_COMMIT_FROM_REF).ok();
    let to_ref = EnvVars.var(EnvVars::PRE_COMMIT_TO_REF).ok();

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

    Ok(Some("HEAD^!".to_string()))
}
