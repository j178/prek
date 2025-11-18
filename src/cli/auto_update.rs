use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use bstr::ByteSlice;
use futures::StreamExt;
use itertools::Itertools;
use lazy_regex::regex;
use owo_colors::OwoColorize;
use prek_consts::MANIFEST_FILE;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;
use serde::Serializer;
use serde::ser::SerializeMap;
use tracing::trace;

use crate::cli::ExitStatus;
use crate::cli::reporter::AutoUpdateReporter;
use crate::config::{RemoteRepo, Repo};
use crate::fs::CWD;
use crate::printer::Printer;
use crate::run::CONCURRENCY;
use crate::store::Store;
use crate::workspace::{Project, Workspace};
use crate::{config, git};

#[derive(Default, Clone)]
struct Revision {
    rev: String,
    frozen: Option<String>,
}

pub(crate) async fn auto_update(
    store: &Store,
    config: Option<PathBuf>,
    filter_repos: Vec<String>,
    bleeding_edge: bool,
    freeze: bool,
    jobs: usize,
    dry_run: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    struct RepoInfo<'a> {
        project: &'a Project,
        remote_size: usize,
        remote_index: usize,
    }

    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    // TODO: support selectors?
    let workspace = Workspace::discover(store, workspace_root, config, None, true)?;

    // Collect repos and deduplicate by RemoteRepo
    #[allow(clippy::mutable_key_type)]
    let mut repo_updates: FxHashMap<&RemoteRepo, Vec<RepoInfo>> = FxHashMap::default();

    for project in workspace.projects() {
        let remote_size = project
            .config()
            .repos
            .iter()
            .filter(|r| matches!(r, Repo::Remote(_)))
            .count();

        let mut remote_index = 0;
        for repo in &project.config().repos {
            if let Repo::Remote(remote_repo) = repo {
                let updates = repo_updates.entry(remote_repo).or_default();
                updates.push(RepoInfo {
                    project,
                    remote_size,
                    remote_index,
                });
                remote_index += 1;
            }
        }
    }

    let jobs = if jobs == 0 { *CONCURRENCY } else { jobs };
    let jobs = jobs
        .min(if filter_repos.is_empty() {
            repo_updates.len()
        } else {
            filter_repos.len()
        })
        .max(1);

    let reporter = AutoUpdateReporter::from(printer);

    let mut tasks = futures::stream::iter(repo_updates.iter().filter(|(remote_repo, _)| {
        // Filter by user specified repositories
        if filter_repos.is_empty() {
            true
        } else {
            filter_repos.iter().any(|r| r == remote_repo.repo.as_str())
        }
    }))
    .map(async |(remote_repo, _)| {
        let progress = reporter.on_update_start(&remote_repo.to_string());

        let result = update_repo(remote_repo, bleeding_edge, freeze).await;

        reporter.on_update_complete(progress);

        (*remote_repo, result)
    })
    .buffer_unordered(jobs)
    .collect::<Vec<_>>()
    .await;

    // Sort tasks by repository URL for consistent output order
    tasks.sort_by(|(a, _), (b, _)| a.repo.cmp(&b.repo));

    reporter.on_complete();

    // Group results by project config file
    #[allow(clippy::mutable_key_type)]
    let mut project_updates: FxHashMap<&Project, Vec<Option<Revision>>> = FxHashMap::default();
    let mut failure = false;

    for (remote_repo, result) in tasks {
        match result {
            Ok(new_rev) => {
                if remote_repo.rev == new_rev.rev {
                    writeln!(
                        printer.stdout(),
                        "[{}] already up to date",
                        remote_repo.repo.as_str().yellow()
                    )?;
                } else {
                    writeln!(
                        printer.stdout(),
                        "[{}] updating {} -> {}",
                        remote_repo.repo.as_str().cyan(),
                        remote_repo.rev,
                        new_rev.rev
                    )?;
                }

                // Apply this update to all projects that reference this repo
                if let Some(projects) = repo_updates.get(&remote_repo) {
                    for RepoInfo {
                        project,
                        remote_size,
                        remote_index,
                    } in projects
                    {
                        let revisions = project_updates
                            .entry(project)
                            .or_insert_with(|| vec![None; *remote_size]);
                        revisions[*remote_index] = Some(new_rev.clone());
                    }
                }
            }
            Err(e) => {
                failure = true;
                writeln!(
                    printer.stderr(),
                    "[{}] update failed: {e}",
                    remote_repo.repo.as_str().red()
                )?;
            }
        }
    }

    if !dry_run {
        // Update each project config file
        for (project, revisions) in project_updates {
            let has_changes = revisions.iter().any(Option::is_some);
            if has_changes {
                write_new_config(project.config_file(), &revisions).await?;
            }
        }
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
        cmd.arg("--exact-match")
    } else {
        // `--abbrev=0` suppress long format, find the closest tag name without any suffix
        cmd.arg("--abbrev=0")
    };

    let output = cmd.output().await?;
    let mut rev = if output.status.success() {
        let rev = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let rev = get_best_candidate_tag(tmp_dir.path(), &rev, &repo.rev)
            .await
            .unwrap_or(rev);
        trace!("Using best candidate tag `{rev}`");
        rev
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
    trace!("Resolved latest tag to `{rev}`");

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

    // Workaround for Windows: https://github.com/pre-commit/pre-commit/issues/2865,
    // https://github.com/j178/prek/issues/614
    if cfg!(windows) {
        git::git_cmd("git show")?
            .arg("show")
            .arg(format!("{rev}:{MANIFEST_FILE}"))
            .current_dir(tmp_dir.path())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await?;
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

/// Multiple tags can exist on an SHA. Sometimes a moving tag is attached
/// to a version tag. Try to pick the tag that looks like a version and most similar
/// to the current revision.
async fn get_best_candidate_tag(repo: &Path, rev: &str, current_rev: &str) -> Result<String> {
    let stdout = git::git_cmd("git tag")?
        .arg("tag")
        .arg("--points-at")
        .arg(format!("{rev}^{{}}"))
        .check(true)
        .current_dir(repo)
        .output()
        .await?
        .stdout;

    String::from_utf8_lossy(&stdout)
        .lines()
        .filter(|line| line.contains('.'))
        .sorted_by_key(|tag| {
            // Prefer tags that are more similar to the current revision
            levenshtein::levenshtein(tag, current_rev)
        })
        .next()
        .map(ToString::to_string)
        .ok_or_else(|| anyhow::anyhow!("No tags found for revision {rev}"))
}

async fn write_new_config(path: &Path, revisions: &[Option<Revision>]) -> Result<()> {
    let mut lines = fs_err::tokio::read_to_string(path)
        .await?
        .split_inclusive('\n')
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    let rev_regex = regex!(r#"^(\s+)rev:(\s*)(['"]?)([^\s#]+)(.*)(\r?\n)$"#);

    let rev_lines = lines
        .iter()
        .enumerate()
        .filter_map(|(line_no, line)| {
            if rev_regex.is_match(line) {
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
            // This repo was not updated, skip
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
            .context("Failed to capture rev line")?;

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
