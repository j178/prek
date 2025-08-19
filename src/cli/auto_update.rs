use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use futures::StreamExt;
use owo_colors::OwoColorize;

use crate::cli::ExitStatus;
use crate::cli::reporter::AutoUpdateReporter;
use crate::config::{MANIFEST_FILE, RemoteRepo, Repo};
use crate::fs::CWD;
use crate::printer::Printer;
use crate::run::CONCURRENCY;
use crate::workspace::Project;
use crate::{config, git};

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
        .filter(|repo| {
            if repos.is_empty() {
                true
            } else {
                repos.iter().any(|r| r == repo.repo.as_str())
            }
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

    let mut tasks = futures::stream::iter(&config_repos)
        .enumerate()
        .map(async |(idx, repo)| {
            let progress = reporter.on_update_start(&repo.to_string());

            let result = update_repo(repo, bleeding_edge, freeze).await;

            reporter.on_update_complete(progress);

            (idx, result)
        })
        .buffer_unordered(jobs);

    let mut revisions = Vec::new();

    let mut failure = false;
    while let Some((idx, result)) = tasks.next().await {
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
                    revisions.push((idx, new));
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

    if failure {
        return Ok(ExitStatus::Failure);
    }
    Ok(ExitStatus::Success)
}

#[derive(Default, Clone)]
struct Revision {
    rev: String,
    frozen: Option<String>,
    hook_ids: Vec<String>,
}

async fn update_repo(repo: &RemoteRepo, bleeding_edge: bool, freeze: bool) -> Result<Revision> {
    let tmp_dir = tempfile::tempdir()?;

    git::init_repo(repo.repo.as_str(), tmp_dir.path()).await?;
    git::git_cmd("git config")?
        .arg("config")
        .arg("extensions.partialClone")
        .arg("true")
        .current_dir(tmp_dir.path())
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

    if !bleeding_edge {
        rev = get_best_candidate_tag(tmp_dir.path(), &rev)
            .await
            .unwrap_or(rev);
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
        .status()
        .await?;

    let manifest = config::read_manifest(tmp_dir.path())?;

    let new_revision = Revision {
        rev,
        frozen,
        hook_ids: manifest.hooks.into_iter().map(|h| h.id).collect(),
    };

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
