use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use prek_consts::PREK_TOML;
use strum::IntoEnumIterator;
use tempfile::TempDir;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Value};

use crate::cli::run::Selectors;
use crate::cli::{ExitStatus, RunArgs, RunOptions};
use crate::config::{self, Stage};
use crate::git::{self, GitCommandExt, TerminalPrompt};
use crate::hooks::{BuiltinHooks, MetaHooks};
use crate::printer::Printer;
use crate::store::{Store, StoreWriteGuard};
use crate::warn_user;
use prek_consts::env_vars::EnvVars;

async fn resolve_commit(repo: &Path, rev: &str) -> Result<String> {
    let output = git::git_cmd()?
        .current_dir(repo)
        .sanitize_git_repo_env()
        .args(["rev-parse", "--verify", "--end-of-options"])
        .arg(format!("{rev}^{{commit}}"))
        .output()
        .await?;
    Ok(str::from_utf8(output.stdout.trim_ascii())?.to_owned())
}

async fn remote_head(source: &str) -> Result<String> {
    let output = git::git_cmd()?
        .args(["ls-remote", "--exit-code", "--"])
        .hidden_args([source])
        .arg("HEAD")
        .output()
        .await?;
    let (sha, _) = str::from_utf8(&output.stdout)?
        .split_once('\t')
        .context("Git did not report a remote HEAD")?;
    Ok(sha.to_owned())
}

async fn snapshot_local(
    repo_path: &Path,
    scratch: &Path,
    prompt: TerminalPrompt,
) -> Result<TempDir> {
    let temp = tempfile::tempdir_in(scratch)?;
    let shadow = temp.path();
    git::git_cmd()?
        .sanitize_git_repo_env()
        .args(["clone", "--no-checkout", "--"])
        .arg(repo_path)
        .arg(shadow)
        .output()
        .await?;
    git::git_cmd()?
        .current_dir(shadow)
        .sanitize_git_repo_env()
        .args(["read-tree", "HEAD"])
        .output()
        .await?;

    let index_path = shadow.join(".git/index");
    let objects_path = shadow.join(".git/objects");

    let staged_files = git::staged_files(repo_path, false).await?;
    if !staged_files.is_empty() {
        git::git_cmd()?
            .arg("add")
            .arg("--")
            .file_args(staged_files.iter().map(|file| &file.path))
            .current_dir(repo_path)
            .sanitize_git_repo_env()
            .env("GIT_INDEX_FILE", &index_path)
            .env("GIT_OBJECT_DIRECTORY", &objects_path)
            .output()
            .await?;
    }

    let mut add_u_cmd = git::git_cmd()?;
    add_u_cmd
        .arg("add")
        .arg("--update") // Update tracked files
        .current_dir(repo_path)
        .sanitize_git_repo_env()
        .env("GIT_INDEX_FILE", &index_path)
        .env("GIT_OBJECT_DIRECTORY", &objects_path)
        .output()
        .await?;

    // A fixed identity and timestamp give identical working trees the same snapshot.
    git::git_cmd()?
        .sanitize_git_repo_env()
        .arg("commit")
        .arg("-m")
        .arg("Temporary commit by prek try-repo")
        .arg("--no-gpg-sign")
        .arg("--no-edit")
        .arg("--no-verify")
        .current_dir(shadow)
        .env("GIT_AUTHOR_NAME", "prek test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "prek test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", "2000-01-01T00:00:00Z")
        .env("GIT_COMMITTER_DATE", "2000-01-01T00:00:00Z")
        .output()
        .await?;

    // The index came from the author's working tree. Materialize it in the
    // temporary checkout before publishing that checkout to the shared store.
    git::git_cmd()?
        .current_dir(shadow)
        .sanitize_git_repo_env()
        .args(["checkout", "--force", "--detach", "HEAD"])
        .env(EnvVars::PREK_INTERNAL__SKIP_POST_CHECKOUT, "1")
        .output()
        .await?;
    git::update_submodules(shadow, prompt, true).await?;
    Ok(temp)
}

async fn prepare_repo(
    store: &StoreWriteGuard<'_>,
    repo: &str,
    rev: Option<&str>,
) -> Result<(config::RemoteRepo, PathBuf)> {
    let path = Path::new(repo);
    let name = rev.unwrap_or("HEAD");
    let prompt = if EnvVars::is_under_ci() {
        TerminalPrompt::Disabled
    } else {
        TerminalPrompt::Enabled
    };
    let (source, sha, checkout) = if path.is_dir() {
        let path = dunce::canonicalize(path)?;
        let source = path.to_string_lossy().into_owned();
        if rev.is_none() && git::has_diff("HEAD", &path).await? {
            warn_user!("Local repository has uncommitted changes. Creating a temporary copy...");
            let checkout = snapshot_local(&path, &store.scratch_path(), prompt).await?;
            let sha = resolve_commit(checkout.path(), "HEAD").await?;
            (source, sha, Some(checkout))
        } else {
            (source, resolve_commit(&path, name).await?, None)
        }
    } else if name == "HEAD" {
        (repo.to_owned(), remote_head(repo).await?, None)
    } else if matches!(name.len(), 40 | 64) && config::looks_like_sha(name) {
        (repo.to_owned(), name.to_ascii_lowercase(), None)
    } else {
        // Let Git interpret branch names, annotated tags and revision expressions.
        // Publish the fetched checkout under its exact commit so a moving ref
        // never selects the checkout or environment from a previous invocation.
        let checkout = store.fetch_revision(repo, name, prompt).await?;
        let sha = resolve_commit(checkout.path(), "HEAD").await?;
        (repo.to_owned(), sha, Some(checkout))
    };

    let pinned = config::RemoteRepo::new(source, sha, Vec::new());
    let path = match checkout {
        Some(temp) => store.persist_cloned_repo(&pinned, temp).await?,
        None => store.clone_repo(&pinned, None).await?,
    };
    Ok((pinned, path))
}

fn render_repo_config_toml(repo_path: &str, rev: Option<&str>, hooks: &[String]) -> String {
    let mut doc = DocumentMut::new();
    let mut repo_table = toml_edit::Table::new();
    repo_table["repo"] = toml_edit::value(repo_path);
    if let Some(rev) = rev {
        repo_table["rev"] = toml_edit::value(rev);
    }

    let mut hooks_array = Array::new();
    hooks_array.set_trailing_comma(true);
    hooks_array.set_trailing("\n");
    for hook_id in hooks {
        let mut hook_table = InlineTable::new();
        hook_table.insert("id", hook_id.into());
        let mut value = Value::InlineTable(hook_table);
        value.decor_mut().set_prefix("\n  ");
        hooks_array.push(value);
    }
    repo_table.insert("hooks", Item::Value(Value::Array(hooks_array)));

    let mut repos = ArrayOfTables::new();
    repos.push(repo_table);
    doc["repos"] = Item::ArrayOfTables(repos);

    doc.to_string()
}

fn select_predefined_hooks<H>(selectors: &Selectors) -> Vec<String>
where
    H: AsRef<str> + IntoEnumIterator,
{
    H::iter()
        .filter(|hook| selectors.matches_hook_id(hook.as_ref()))
        .map(|hook| hook.as_ref().to_string())
        .collect()
}

pub(crate) async fn try_repo(
    store: &Store,
    config: Option<PathBuf>,
    repo: String,
    rev: Option<String>,
    run_args: RunOptions,
    stage: Option<Stage>,
    refresh: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    if config.is_some() {
        warn_user!("`--config` option is ignored when using `try-repo`");
    }

    let tmp_dir = TempDir::with_prefix_in("try-repo-", store.scratch_path())?;

    let selectors = Selectors::load(&run_args.includes, &run_args.skips, git::root()?)?;

    let predefined_hooks = match repo.as_str() {
        "builtin" => Some(select_predefined_hooks::<BuiltinHooks>(&selectors)),
        "meta" => Some(select_predefined_hooks::<MetaHooks>(&selectors)),
        _ => None,
    };

    let (config_str, display_config_str) = if let Some(hooks) = predefined_hooks {
        if rev.is_some() {
            warn_user!("`--rev` option is ignored for the `{repo}` repo");
        }
        (render_repo_config_toml(&repo, None, &hooks), None)
    } else {
        let (pinned, repo_clone_path) = {
            let guard = store.write_lock().await?;
            prepare_repo(&guard, &repo, rev.as_deref()).await?
        };
        let manifest =
            config::read_manifest(&repo_clone_path.join(prek_consts::PRE_COMMIT_HOOKS_YAML))?;
        let hooks = manifest
            .hooks
            .into_iter()
            .filter(|hook| selectors.matches_hook_id(&hook.id))
            .map(|hook| hook.id)
            .collect::<Vec<_>>();

        let config_str = render_repo_config_toml(pinned.source(), Some(&pinned.rev), &hooks);
        let display_rev = rev.as_deref().unwrap_or(&pinned.rev);
        let display_config_str = if pinned.source() != repo || display_rev != pinned.rev {
            Some(render_repo_config_toml(&repo, Some(display_rev), &hooks))
        } else {
            None
        };
        (config_str, display_config_str)
    };
    let config_file = tmp_dir.path().join(PREK_TOML);
    fs_err::tokio::write(&config_file, &config_str).await?;

    let display_config_str = display_config_str.as_deref().unwrap_or(&config_str);

    writeln!(
        printer.stdout(),
        "{}",
        format!("Using generated `{PREK_TOML}`:").cyan().bold()
    )?;
    writeln!(printer.stdout(), "{}", display_config_str.dimmed())?;

    let mut run_args = run_args;
    // The generated config already contains only the hooks selected above.
    run_args.includes.clear();
    run_args.skips.clear();

    crate::cli::run(
        store,
        Some(config_file),
        RunArgs {
            options: run_args,
            stage,
            ..RunArgs::default()
        },
        refresh,
        verbose,
        printer,
    )
    .await
}
