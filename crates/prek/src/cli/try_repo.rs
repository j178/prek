use std::borrow::Cow;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use itertools::Itertools;
use owo_colors::OwoColorize;
use tempfile::TempDir;

use crate::cli::ExitStatus;
use crate::cli::run::Selectors;
use crate::config;
use crate::git;
use crate::git::GIT_ROOT;
use crate::printer::Printer;
use crate::store::Store;
use crate::warn_user;

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

async fn clone_and_commit(repo_path: &Path, head_rev: &str, tmp_dir: &Path) -> Result<PathBuf> {
    let shadow = tmp_dir.join("shadow-repo");
    git::git_cmd("clone shadow repo")?
        .arg("clone")
        .arg(repo_path)
        .arg(&shadow)
        .output()
        .await?;
    git::git_cmd("checkout shadow repo")?
        .arg("checkout")
        .arg(head_rev)
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
            .current_dir(repo_path)
            .env("GIT_INDEX_FILE", &index_path)
            .env("GIT_OBJECT_DIRECTORY", &objects_path)
            .output()
            .await?;
    }

    let mut add_u_cmd = git::git_cmd("add unstaged to shadow")?;
    add_u_cmd
        .arg("add")
        .arg("--update") // Update tracked files
        .current_dir(repo_path)
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
        .current_dir(&shadow)
        .env("GIT_AUTHOR_NAME", "prek test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "prek test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .output()
        .await?;

    Ok(shadow)
}

async fn prepare_repo_and_rev<'a>(
    repo: &'a str,
    rev: Option<&'a str>,
    tmp_dir: &'a Path,
) -> Result<(Cow<'a, str>, String)> {
    let repo_path = Path::new(repo);
    let is_local = repo_path.is_dir();

    // If rev is provided, use it directly.
    if let Some(rev) = rev {
        return Ok((Cow::Borrowed(repo), rev.to_string()));
    }

    // Get HEAD revision
    let head_rev = if is_local {
        get_head_rev(repo_path).await?
    } else {
        // For remote repositories, use ls-remote
        let head_rev = git::git_cmd("get head rev")?
            .arg("ls-remote")
            .arg("--exit-code")
            .arg(repo)
            .arg("HEAD")
            .output()
            .await?
            .stdout;
        String::from_utf8_lossy(&head_rev)
            .split_ascii_whitespace()
            .next()
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to parse HEAD revision from git ls-remote output")
            })?
            .to_string()
    };

    // If repo is a local repo with uncommitted changes, create a shadow repo to commit the changes.
    if is_local && git::has_diff("HEAD", repo_path).await? {
        warn_user!("Creating temporary repo with uncommitted changes...");
        let shadow = clone_and_commit(repo_path, &head_rev, tmp_dir).await?;
        let head_rev = get_head_rev(&shadow).await?;
        Ok((Cow::Owned(shadow.to_string_lossy().to_string()), head_rev))
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

    let store = Store::from_settings()?;
    let tmp_dir = TempDir::with_prefix_in("try-repo-", store.scratch_path())?;

    let (repo_path, rev) = prepare_repo_and_rev(&repo, rev.as_deref(), tmp_dir.path())
        .await
        .context("Failed to determine repository and revision")?;

    let store = Store::from_path(tmp_dir.path()).init()?;
    let repo_clone_path = store
        .clone_repo(
            &config::RemoteRepo::new(repo_path.to_string(), rev.clone(), vec![]),
            None,
        )
        .await?;

    let selectors = Selectors::load(&run_args.includes, &run_args.skips, GIT_ROOT.as_ref()?)?;

    let manifest =
        config::read_manifest(&repo_clone_path.join(prek_consts::PRE_COMMIT_HOOKS_YAML))?;
    let hooks_str = manifest
        .hooks
        .into_iter()
        .filter(|hook| selectors.matches_hook_id(&hook.id))
        .map(|hook| format!("{}- id: {}", " ".repeat(6), hook.id))
        .join("\n");

    let config_str = indoc::formatdoc! {r"
    repos:
      - repo: {repo_path}
        rev: {rev}
        hooks:
    {hooks_str}
    ",
        repo_path = repo_path,
        rev = rev,
        hooks_str = hooks_str,
    };

    let config_file = tmp_dir.path().join(prek_consts::PRE_COMMIT_HOOKS_YAML);
    fs_err::tokio::write(&config_file, &config_str).await?;

    writeln!(printer.stdout(), "{}", "Using config:".cyan().bold())?;
    write!(printer.stdout(), "{}", config_str.dimmed())?;

    crate::cli::run(
        &store,
        Some(config_file),
        vec![],
        vec![],
        run_args.hook_stage,
        run_args.from_ref,
        run_args.to_ref,
        run_args.all_files,
        run_args.files,
        run_args.directory,
        run_args.last_commit,
        run_args.show_diff_on_failure,
        run_args.fail_fast,
        run_args.dry_run,
        refresh,
        run_args.extra,
        verbose,
        printer,
    )
    .await
}
