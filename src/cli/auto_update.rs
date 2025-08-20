use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use bstr::ByteSlice;
use fancy_regex::Regex;
use futures::StreamExt;
use itertools::Itertools;
use owo_colors::OwoColorize;
use rustc_hash::FxHashSet;
use serde::Serializer;
use serde::ser::SerializeMap;
use tracing::trace;

use crate::cli::ExitStatus;
use crate::cli::reporter::AutoUpdateReporter;
use crate::config::{MANIFEST_FILE, RemoteRepo, Repo};
use crate::fs::CWD;
use crate::printer::Printer;
use crate::run::CONCURRENCY;
use crate::workspace::Project;
use crate::{config, git};

#[derive(Default, Clone)]
struct Revision {
    rev: String,
    frozen: Option<String>,
}

pub(crate) async fn auto_update(
    config: Option<PathBuf>,
    repos: Vec<String>,
    bleeding_edge: bool,
    freeze: bool,
    jobs: usize,
    printer: Printer,
) -> Result<ExitStatus> {
    // TODO: update whole workspace?
    let project = Project::from_config_file_or_directory(config, &CWD)?;

    let config_repos = project
        .config()
        .repos
        .iter()
        .filter_map(|repo| match repo {
            Repo::Remote(repo) => Some(repo),
            _ => None,
        })
        .collect::<Vec<_>>();

    let jobs = if jobs == 0 { *CONCURRENCY } else { jobs };
    let jobs = jobs
        .min(if repos.is_empty() {
            config_repos.len()
        } else {
            repos.len()
        })
        .max(1);

    let reporter = AutoUpdateReporter::from(printer);

    let mut tasks = futures::stream::iter(config_repos.iter().enumerate().filter(|(_, repo)| {
        // Filter by user specified repositories
        if repos.is_empty() {
            true
        } else {
            repos.iter().any(|r| r == repo.repo.as_str())
        }
    }))
    .map(async |(idx, repo)| {
        let progress = reporter.on_update_start(&repo.to_string());

        let result = update_repo(repo, bleeding_edge, freeze).await;

        reporter.on_update_complete(progress);

        (idx, result)
    })
    .buffer_unordered(jobs)
    .collect::<Vec<_>>()
    .await;

    tasks.sort_by_key(|(idx, _)| *idx);

    reporter.on_complete();

    let mut revisions = vec![None; config_repos.len()];
    let mut failure = false;
    let mut changed = false;

    for (idx, result) in tasks {
        let old = config_repos[idx];
        match result {
            Ok(new) => {
                if old.rev == new.rev {
                    writeln!(
                        printer.stdout(),
                        "[{}] already up to date",
                        old.repo.as_str().yellow()
                    )?;
                } else {
                    writeln!(
                        printer.stdout(),
                        "[{}] updating {} -> {}",
                        old.repo.as_str().cyan(),
                        old.rev,
                        new.rev
                    )?;
                    changed = true;
                    revisions[idx] = Some(new);
                }
            }
            Err(e) => {
                failure = true;
                writeln!(
                    printer.stderr(),
                    "[{}] update failed: {e}",
                    old.repo.as_str().red()
                )?;
            }
        }
    }

    if changed {
        write_new_config(project.config_file(), &revisions).await?;
    }

    if failure {
        return Ok(ExitStatus::Failure);
    }
    Ok(ExitStatus::Success)
}

async fn update_repo(repo: &RemoteRepo, bleeding_edge: bool, freeze: bool) -> Result<Revision> {
    let tmp_dir = tempfile::tempdir()?;

    trace!(
        "Cloning repository `{}` to `{}`",
        repo.repo,
        tmp_dir.path().display()
    );

    git::init_repo(repo.repo.as_str(), tmp_dir.path()).await?;
    git::git_cmd("git config")?
        .arg("config")
        .arg("extensions.partialClone")
        .arg("true")
        .current_dir(tmp_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;
    git::git_cmd("git fetch")?
        .arg("fetch")
        .arg("origin")
        .arg("HEAD")
        .arg("--quiet")
        .arg("--filter=blob:none")
        .arg("--tags")
        .current_dir(tmp_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    let mut cmd = git::git_cmd("git describe")?;
    cmd.arg("describe")
        .arg("FETCH_HEAD")
        .arg("--tags") // use any tags found in refs/tags
        .check(false)
        .current_dir(tmp_dir.path());
    if bleeding_edge {
        cmd.arg("--exact")
    } else {
        cmd.arg("--abbrev=0") // find the closest tag name without any suffix
    };

    let output = cmd.output().await?;
    let mut rev = if output.status.success() {
        String::from_utf8_lossy(&output.stdout).trim().to_string()
    } else {
        trace!("Failed to describe FETCH_HEAD, using rev-parse instead");
        // "fatal: no tag exactly matches xxx"
        let stdout = git::git_cmd("git rev-parse")?
            .arg("rev-parse")
            .arg("FETCH_HEAD")
            .check(true)
            .current_dir(tmp_dir.path())
            .output()
            .await?
            .stdout;
        String::from_utf8_lossy(&stdout).trim().to_string()
    };
    trace!("Resolved FETCH_HEAD to `{rev}`");

    if !bleeding_edge {
        rev = get_best_candidate_tag(tmp_dir.path(), &rev)
            .await
            .unwrap_or(rev);
        trace!("Using best candidate tag `{rev}` for revision");
    }

    let mut frozen = None;
    if freeze {
        let exact = git::git_cmd("git rev-parse")?
            .arg("rev-parse")
            .arg(&rev)
            .current_dir(tmp_dir.path())
            .output()
            .await?
            .stdout;
        let exact = String::from_utf8_lossy(&exact).trim().to_string();
        if rev != exact {
            trace!("Freezing revision to `{exact}`");
            frozen = Some(rev);
            rev = exact;
        }
    }

    git::git_cmd("git checkout")?
        .arg("checkout")
        .arg("--quiet")
        .arg(&rev)
        .arg("--")
        .arg(MANIFEST_FILE)
        .current_dir(tmp_dir.path())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .await?;

    let manifest = config::read_manifest(&tmp_dir.path().join(MANIFEST_FILE))?;
    let new_hook_ids = manifest
        .hooks
        .into_iter()
        .map(|h| h.id)
        .collect::<FxHashSet<_>>();
    let hooks_missing = repo
        .hooks
        .iter()
        .filter(|h| !new_hook_ids.contains(&h.id))
        .map(|h| h.id.clone())
        .collect::<Vec<_>>();
    if !hooks_missing.is_empty() {
        return Err(anyhow::anyhow!(
            "Cannot update to rev `{}`, hook{} {} missing: {}",
            rev,
            if hooks_missing.len() > 1 { "s" } else { "" },
            if hooks_missing.len() > 1 { "are" } else { "is" },
            hooks_missing.join(", ")
        ));
    }

    let new_revision = Revision { rev, frozen };

    Ok(new_revision)
}

/// Multiple tags can exist on a SHA. Sometimes a moving tag is attached
/// to a version tag. Try to pick the tag that looks like a version.
async fn get_best_candidate_tag(repo: &Path, rev: &str) -> Result<String> {
    let stdout = git::git_cmd("git tag")?
        .arg("tag")
        .arg("--points-at")
        .arg(rev)
        .check(true)
        .current_dir(repo)
        .output()
        .await?
        .stdout;

    String::from_utf8_lossy(&stdout)
        .lines()
        .filter(|line| line.contains('.'))
        .map(ToString::to_string)
        .next()
        .ok_or_else(|| anyhow::anyhow!("No tags found for revision {}", rev))
}

async fn write_new_config(path: &Path, revisions: &[Option<Revision>]) -> Result<()> {
    let mut lines = fs_err::tokio::read_to_string(path)
        .await?
        .split_inclusive('\n')
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    let rev_regex = Regex::new(r#"^(\s+)rev:(\s*)(['"]?)([^\s#]+)(.*)(\r?\n)$"#)
        .expect("Failed to compile regex");

    let rev_lines = lines
        .iter()
        .enumerate()
        .filter_map(|(line_no, line)| {
            if let Ok(true) = rev_regex.is_match(line) {
                Some(line_no)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();

    if rev_lines.len() != revisions.len() {
        anyhow::bail!(
            "Found {} `rev:` lines in `{}` but expected {}, file content may have changed",
            rev_lines.len(),
            path.display(),
            revisions.len()
        );
    }

    for (line_no, revision) in rev_lines.iter().zip_eq(revisions) {
        let Some(revision) = revision else {
            continue;
        };

        let mut new_rev = Vec::new();
        let mut serializer = serde_yaml::Serializer::new(&mut new_rev);
        serializer
            .serialize_map(Some(1))?
            .serialize_entry("rev", &revision.rev)?;
        serializer.end()?;

        let (_, new_rev) = new_rev
            .to_str()?
            .split_once(':')
            .expect("Failed to split serialized revision");

        let caps = rev_regex
            .captures(&lines[*line_no])
            .expect("Invalid regex")
            .expect("Failed to capture revision line");

        // TODO: preserve the quote style
        // Naively add the original quotes
        let new_rev = if !caps[3].is_empty() && !new_rev.contains(&caps[3]) {
            format!("{}{}{}", &caps[3], new_rev.trim(), &caps[3])
        } else {
            new_rev.trim().to_string()
        };

        let comment = if let Some(frozen) = &revision.frozen {
            format!("  # frozen: {frozen}")
        } else if caps[5].trim().starts_with("# frozen:") {
            String::new()
        } else {
            caps[5].to_string()
        };

        lines[*line_no] = format!(
            "{}rev:{}{}{}{}",
            &caps[1],
            &caps[2],
            new_rev.trim(),
            comment,
            &caps[6]
        );
    }

    fs_err::tokio::write(path, lines.join("").as_bytes())
        .await
        .with_context(|| format!("Failed to write updated config file `{}`", path.display()))?;

    Ok(())
}
