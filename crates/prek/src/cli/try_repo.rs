use std::borrow::Cow;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use owo_colors::OwoColorize;
use prek_consts::PREK_TOML;
use tempfile::TempDir;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Value};

use crate::cli::ExitStatus;
use crate::cli::run::Selectors;
use crate::config;
use crate::git;
use crate::git::GIT_ROOT;
use crate::hooks::{BuiltinHooks, HookRegistry, MetaHooks};
use crate::printer::Printer;
use crate::store::Store;
use crate::warn_user;

enum RepoType<'a> {
    Git { repo: &'a str, rev: Option<&'a str> },
    Builtin,
    Meta,
}

struct TryRepoContext {
    store: Store,
    tmp_dir: TempDir,
}

impl TryRepoContext {
    fn new() -> Result<Self> {
        let store = Store::from_settings()?;
        let tmp_dir = TempDir::with_prefix_in("try-repo-", store.scratch_path())?;
        let store = Store::from_path(tmp_dir.path()).init()?;
        Ok(Self { store, tmp_dir })
    }

    async fn run_with_config(
        self,
        config_str: &str,
        run_args: crate::cli::RunArgs,
        refresh: bool,
        verbose: bool,
        printer: Printer,
    ) -> Result<ExitStatus> {
        let config_file = self.tmp_dir.path().join(PREK_TOML);
        fs_err::tokio::write(&config_file, config_str).await?;

        writeln!(
            printer.stdout(),
            "{}",
            format!("Using generated `{PREK_TOML}`:").cyan().bold()
        )?;
        writeln!(printer.stdout(), "{}", config_str.dimmed())?;

        crate::cli::run(
            &self.store,
            Some(config_file),
            vec![],
            vec![],
            run_args.stage,
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
}

async fn get_head_rev(repo: &Path) -> Result<String> {
    let head_rev = git::git_cmd("get head rev")?
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo)
        .output()
        .await?
        .stdout;
    Ok(String::from_utf8_lossy(&head_rev).trim().to_string())
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

    git::git_cmd("add unstaged to shadow")?
        .arg("add")
        .arg("--update")
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
        let output = git::git_cmd("get head rev")?
            .arg("ls-remote")
            .arg("--exit-code")
            .arg(repo)
            .arg("HEAD")
            .output()
            .await?
            .stdout;
        String::from_utf8_lossy(&output)
            .split_ascii_whitespace()
            .next()
            .ok_or_else(|| {
                anyhow::anyhow!("Failed to parse HEAD revision from git ls-remote output")
            })?
            .to_string()
    };

    // If repo is a local repo with uncommitted changes, create a shadow repo.
    if is_local && git::has_diff("HEAD", repo_path).await? {
        warn_user!("Creating temporary repo with uncommitted changes...");
        let shadow = clone_and_commit(repo_path, &head_rev, tmp_dir).await?;
        let head_rev = get_head_rev(&shadow).await?;
        Ok((Cow::Owned(shadow.to_string_lossy().to_string()), head_rev))
    } else {
        Ok((Cow::Borrowed(repo), head_rev))
    }
}

fn render_repo_config_toml(repo_path: &str, rev: Option<&str>, hooks: Vec<String>) -> String {
    let mut doc = DocumentMut::new();
    let mut repo_table = toml_edit::Table::new();
    // Normalize path separators so toml_edit produces consistent quoting across platforms
    repo_table["repo"] = toml_edit::value(repo_path.replace('\\', "/"));
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

    let repo_type = if repo.eq_ignore_ascii_case("builtin") {
        if rev.is_some() {
            warn_user!("`--ref` option is ignored for `builtin` repo");
        }
        RepoType::Builtin
    } else if repo.eq_ignore_ascii_case("meta") {
        if rev.is_some() {
            warn_user!("`--ref` option is ignored for `meta` repo");
        }
        RepoType::Meta
    } else {
        RepoType::Git {
            repo: &repo,
            rev: rev.as_deref(),
        }
    };

    match repo_type {
        RepoType::Builtin => {
            try_special_repo::<BuiltinHooks>(run_args, refresh, verbose, printer).await
        }
        RepoType::Meta => try_special_repo::<MetaHooks>(run_args, refresh, verbose, printer).await,
        RepoType::Git { repo, rev } => {
            try_git_repo(repo, rev, run_args, refresh, verbose, printer).await
        }
    }
}

async fn try_special_repo<H: HookRegistry>(
    run_args: crate::cli::RunArgs,
    refresh: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let repo_name = H::REPO_NAME;
    let ctx = TryRepoContext::new()?;

    let selectors = Selectors::load(&run_args.includes, &run_args.skips, GIT_ROOT.as_ref()?)?;

    let hook_ids: Vec<_> = H::all_ids()
        .filter(|id| selectors.matches_hook_id(id))
        .collect();

    if hook_ids.is_empty() {
        bail!("No hooks matched the specified selectors for repo `{repo_name}`");
    }

    let hooks = hook_ids.into_iter().map(str::to_string).collect();
    let config_str = render_repo_config_toml(repo_name, None, hooks);

    ctx.run_with_config(&config_str, run_args, refresh, verbose, printer)
        .await
}

async fn try_git_repo(
    repo: &str,
    rev: Option<&str>,
    run_args: crate::cli::RunArgs,
    refresh: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let ctx = TryRepoContext::new()?;

    let (repo_path, rev) = prepare_repo_and_rev(repo, rev, ctx.tmp_dir.path())
        .await
        .context("Failed to determine repository and revision")?;

    let repo_clone_path = ctx
        .store
        .clone_repo(
            &config::RemoteRepo::new(repo_path.to_string(), rev.clone(), vec![]),
            None,
        )
        .await?;

    let selectors = Selectors::load(&run_args.includes, &run_args.skips, GIT_ROOT.as_ref()?)?;

    let manifest =
        config::read_manifest(&repo_clone_path.join(prek_consts::PRE_COMMIT_HOOKS_YAML))?;

    let hooks = manifest
        .hooks
        .into_iter()
        .filter(|hook| selectors.matches_hook_id(&hook.id))
        .map(|hook| hook.id)
        .collect::<Vec<_>>();

    if hooks.is_empty() {
        bail!("No hooks matched the specified selectors for repo `{repo_path}`");
    }

    let config_str = render_repo_config_toml(&repo_path, Some(&rev), hooks);

    ctx.run_with_config(&config_str, run_args, refresh, verbose, printer)
        .await
}
