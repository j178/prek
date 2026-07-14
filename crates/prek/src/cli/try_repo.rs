use std::borrow::Cow;
use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use prek_consts::PREK_TOML;
use tempfile::TempDir;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Value};

use crate::cli::run::Selectors;
use crate::cli::{ExitStatus, RunOptions, flag};
use crate::config::{self, Stage};
use crate::git;
use crate::git::GIT_ROOT;
use crate::printer::Printer;
use crate::store::Store;
use crate::warn_user;

async fn get_head_rev(repo: &Path) -> Result<String> {
    let head_rev = git::git_cmd()?
        .arg("rev-parse")
        .arg("HEAD")
        .current_dir(repo)
        .output()
        .await?
        .stdout;
    let head_rev = String::from_utf8_lossy(&head_rev).trim().to_string();
    Ok(head_rev)
}

struct PreparedRepo<'a> {
    source: Cow<'a, str>,
    rev: String,
}

async fn prepare_repo<'a>(
    store: &Store,
    repo: &'a str,
    rev: Option<&str>,
) -> Result<PreparedRepo<'a>> {
    let repo_path = Path::new(repo);
    let is_local = repo_path.is_dir();
    let runtime_source = if is_local {
        Cow::Owned(
            dunce::canonicalize(repo_path)?
                .to_string_lossy()
                .into_owned(),
        )
    } else {
        Cow::Borrowed(repo)
    };
    let repo_path = Path::new(runtime_source.as_ref());

    // If rev is provided, use it directly.
    if let Some(rev) = rev {
        return Ok(PreparedRepo {
            source: runtime_source,
            rev: rev.to_string(),
        });
    }

    // Get HEAD revision
    let head_rev = if is_local {
        get_head_rev(repo_path).await?
    } else {
        // For remote repositories, use ls-remote
        let head_rev = git::git_cmd()?
            .arg("ls-remote")
            .arg("--exit-code")
            .arg(runtime_source.as_ref())
            .arg("HEAD")
            .output()
            .await?
            .stdout;
        String::from_utf8_lossy(&head_rev)
            .split_ascii_whitespace()
            .next()
            .context("Failed to parse HEAD revision from git ls-remote output")?
            .to_string()
    };

    // Persist a deterministic synthetic commit in the shared source. The logical source remains
    // the canonical local path, so identical dirty trees get the same repo and environment keys.
    if is_local && git::has_diff("HEAD", repo_path).await? {
        warn_user!("Creating temporary repo with uncommitted changes...");
        let source = store.repo_source_path(runtime_source.as_ref());
        let _source_lock = store.repo_source_lock(runtime_source.as_ref()).await?;
        git::ensure_bare_repo(runtime_source.as_ref(), &source).await?;
        let head_rev =
            git::fetch_repo_source_revision(&source, &head_rev, git::TerminalPrompt::Disabled)
                .await?;
        let snapshot =
            git::create_repo_snapshot(&source, repo_path, &head_rev, &store.scratch_path()).await?;
        Ok(PreparedRepo {
            source: runtime_source,
            rev: snapshot,
        })
    } else {
        Ok(PreparedRepo {
            source: runtime_source,
            rev: head_rev,
        })
    }
}

fn render_repo_config_toml(repo_path: &str, rev: &str, hooks: &[String]) -> String {
    let mut doc = DocumentMut::new();
    let mut repo_table = toml_edit::Table::new();
    repo_table["repo"] = toml_edit::value(repo_path);
    repo_table["rev"] = toml_edit::value(rev);

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
    run_args: RunOptions,
    stage: Option<Stage>,
    refresh: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    if config.is_some() {
        warn_user!("`--config` option is ignored when using `try-repo`");
    }

    let store = Store::from_settings()?;
    let selectors = Selectors::load(&run_args.includes, &run_args.skips, GIT_ROOT.as_ref()?)?;

    let (_tmp_dir, prepared, hooks, config_str, config_file) = {
        let _lock = store.lock_async().await?;
        // `cache gc` clears the contents of the store scratch directory after taking the store
        // lock. Keep the active generated config in the system temporary directory so it survives
        // between this preparation lock and the lock acquired by `run`.
        let tmp_dir = TempDir::with_prefix("try-repo-")?;
        let prepared = prepare_repo(&store, &repo, rev.as_deref())
            .await
            .context("Failed to determine repository and revision")?;
        let repo_config =
            config::RemoteRepo::new(prepared.source.to_string(), prepared.rev.clone(), vec![]);
        let repo_clone_path = store.clone_repo(&repo_config, None).await?;
        let manifest =
            config::read_manifest(&repo_clone_path.join(prek_consts::PRE_COMMIT_HOOKS_YAML))?;
        let hooks = manifest
            .hooks
            .into_iter()
            .filter(|hook| selectors.matches_hook_id(&hook.id))
            .map(|hook| hook.id)
            .collect::<Vec<_>>();

        let config_str = render_repo_config_toml(&prepared.source, &prepared.rev, &hooks);
        let config_file = tmp_dir.path().join(PREK_TOML);
        fs_err::tokio::write(&config_file, &config_str).await?;
        // Make the new source/checkout visible to GC before releasing the store lock. `run` also
        // tracks this path, but a concurrent `cache gc` must not sweep a dirty synthetic commit in
        // the interval between preparation and hook initialization.
        store.track_configs(std::iter::once(config_file.as_path()))?;

        (tmp_dir, prepared, hooks, config_str, config_file)
    };

    // The scratch config needs the resolved source, while the displayed config should preserve
    // the user's path so it remains meaningful when copied into their project.
    let display_config_str = if prepared.source.as_ref() == repo {
        Cow::Borrowed(config_str.as_str())
    } else {
        Cow::Owned(render_repo_config_toml(&repo, &prepared.rev, &hooks))
    };

    writeln!(
        printer.stdout(),
        "{}",
        format!("Using generated `{PREK_TOML}`:").cyan().bold()
    )?;
    writeln!(printer.stdout(), "{}", display_config_str.dimmed())?;

    crate::cli::run(
        &store,
        Some(config_file),
        vec![],
        vec![],
        vec![],
        vec![],
        stage,
        run_args.from_ref,
        run_args.to_ref,
        run_args.all_files,
        run_args.files,
        run_args.directory,
        run_args.last_commit,
        run_args.show_diff_on_failure,
        flag(run_args.fail_fast, run_args.no_fail_fast),
        run_args.dry_run,
        refresh,
        run_args.extra,
        verbose,
        printer,
    )
    .await
}
