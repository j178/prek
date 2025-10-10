use std::borrow::Cow;
use std::fmt::Write;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use anyhow::{Context, Result};
use itertools::Itertools;
use owo_colors::OwoColorize;
use tracing::debug;

use crate::cli::ExitStatus;
use crate::cli::run::Selectors;
use crate::config;
use crate::fs::CWD;
use crate::git;
use crate::printer::Printer;
use crate::store::Store;
use crate::warn_user;
use crate::workspace::Workspace;

async fn get_head_rev(repo: &Path) -> Result<String> {
    let head_rev = git::git_cmd("get head rev")?
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo)
        .output()
        .await?
        .stdout;
    let head_rev = String::from_utf8_lossy(&head_rev).trim().to_string();
    Ok(head_rev)
}

async fn prepare_repo_and_rev<'a>(
    repo: &'a str,
    rev: Option<&'a str>,
    tmp_dir: &'a Path,
) -> Result<(Cow<'a, str>, String)> {
    // If rev is provided, use it directly.
    if let Some(rev) = rev {
        return Ok((Cow::Borrowed(repo), rev.to_string()));
    }

    let head_rev = git::git_cmd("get head rev")?
        .arg("ls-remote")
        .arg("--exit-code")
        .arg(repo)
        .arg("HEAD")
        .output()
        .await?
        .stdout;
    let head_rev = String::from_utf8_lossy(&head_rev).trim().to_string();

    // If repo is a local repo with uncommitted changes, create a shadow repo to commit the changes.
    let repo_path = Path::new(repo);
    if repo_path.is_dir() && git::has_diff("HEAD", repo_path).await? {
        debug!("Creating shadow repo for {}", repo);

        let shadow = tmp_dir.join("shadow-repo");
        git::git_cmd("clone shadow repo")?
            .arg("clone")
            .arg(repo)
            .arg(&shadow)
            .output()
            .await?;
        git::git_cmd("checkout shadow repo")?
            .arg("checkout")
            .arg(&head_rev)
            .arg("-b")
            .arg("_prek_tmp")
            .current_dir(&shadow)
            .output()
            .await?;

        let index_path = shadow.join(".git/index");
        let objects_path = shadow.join(".git/objects");

        let staged_files = git::get_staged_files(repo_path).await?;
        if !staged_files.is_empty() {
            git::git_cmd("add staged files to shadow")?
                .arg("add")
                .arg("--")
                .args(&staged_files)
                .current_dir(repo)
                .env("GIT_INDEX_FILE", &index_path)
                .env("GIT_OBJECT_DIRECTORY", &objects_path)
                .output()
                .await?;
        }

        let mut add_u_cmd = git::git_cmd("add unstaged to shadow")?;
        add_u_cmd
            .arg("add")
            .arg("--update") // Update tracked files
            .current_dir(repo)
            .env("GIT_INDEX_FILE", &index_path)
            .env("GIT_OBJECT_DIRECTORY", &objects_path)
            .output()
            .await?;

        git::git_cmd("git commit")?
            .arg("commit")
            .arg("-m")
            .arg("Temporary commit by prek try-repo")
            .arg("--no-gpg-sign")
            .arg("--no-edit")
            .arg("--no-verify")
            .current_dir(repo)
            .env("GIT_AUTHOR_NAME", "prek test")
            .env("GIT_AUTHOR_EMAIL", "test@example.com")
            .env("GIT_COMMITTER_NAME", "prek test")
            .env("GIT_COMMITTER_EMAIL", "test@example.com")
            .output()
            .await?;

        let new_rev = get_head_rev(&shadow).await?;
        Ok((Cow::Owned(shadow.to_string_lossy().to_string()), new_rev))
    } else {
        Ok((Cow::Borrowed(repo), head_rev))
    }
}

pub(crate) async fn try_repo(
    config: Option<PathBuf>,
    repo: String,
    rev: Option<String>,
    run_args: crate::cli::RunArgs,
    refresh: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    if config.is_some() {
        warn_user!("`--config` option is ignored when using `try-repo`");
    }

    let workspace_root = Workspace::find_root(config.as_deref(), &CWD)?;
    let selectors = Selectors::load(&run_args.includes, &run_args.skips, &workspace_root)?;

    let tmp_dir = tempfile::tempdir()?;
    let (repo_path, rev) = prepare_repo_and_rev(&repo, rev.as_deref(), tmp_dir.path())
        .await
        .context("Failed to determine repository and revision")?;

    let store = Store::from_path(tmp_dir.path());
    let repo_clone_path = store
        .clone_repo(
            &config::RemoteRepo {
                repo: repo_path.to_string(),
                rev: rev.clone(),
                hooks: vec![],
            },
            None,
        )
        .await?;

    let hooks = if let Some(hook_id) = hook {
        vec![hook_id]
    } else {
        let manifest = config::read_manifest(&repo_clone_path.join(constants::MANIFEST_FILE))?;
        manifest.hooks.into_iter().map(|h| h.id).collect()
    };

    let hooks_str = hooks
        .iter()
        .map(|hook_id| format!("{}- id: {}", " ".repeat(6), hook_id))
        .join("\n");
    let config_str = indoc::formatdoc! {r"
    repos:
      - repo: {repo_path}
        rev: {rev}
    ",
        repo_path = repo_path,
        rev = rev,
    };

    let config_file = tmp_dir.path().join(constants::CONFIG_FILE);
    fs_err::tokio::write(&config_file, &config_str).await?;

    let mut stdout = printer.stdout();
    writeln!(stdout, "{}", "Using config:".cyan().bold())?;
    write!(stdout, "{}", config_str.dimmed())?;

    crate::cli::run(
        &store,
        Some(config_file),
        vec![], // includes
        vec![], // skips
        run_args.hook_stage,
        run_args.from_ref,
        run_args.to_ref,
        run_args.all_files,
        run_args.files,
        run_args.directory,
        run_args.last_commit,
        run_args.show_diff_on_failure,
        run_args.dry_run,
        refresh,
        run_args.extra,
        verbose,
        printer,
    )
    .await
}
