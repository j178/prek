use std::borrow::Cow;
use std::fmt::Write;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use owo_colors::OwoColorize;
use prek_consts::PREK_TOML;
use seahash::SeaHasher;
use strum::IntoEnumIterator;
use tempfile::TempDir;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Value};

use crate::cli::run::Selectors;
use crate::cli::{ExitStatus, RunArgs, RunOptions};
use crate::config::{self, Stage};
use crate::git;
use crate::hooks::{BuiltinHooks, MetaHooks};
use crate::printer::Printer;
use crate::store::Store;
use crate::warn_user;

/// Resolve `rev` in a local repository to the commit it points to.
async fn resolve_local_rev(repo: &Path, rev: &str) -> Result<String> {
    let output = git::git_cmd()?
        .arg("rev-parse")
        .arg("--verify")
        .arg(format!("{rev}^{{commit}}"))
        .current_dir(repo)
        .output()
        .await?
        .stdout;
    Ok(str::from_utf8(&output)?.trim().to_string())
}

fn is_full_object_id(rev: &str) -> bool {
    matches!(rev.len(), 40 | 64) && rev.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// Resolve `rev` (or `HEAD` when it is `None`) on a remote to the object it points to.
///
/// A `rev` that matches no remote ref is returned unchanged, because it can only be fetched as
/// a commit SHA, which cannot move.
async fn resolve_remote_rev(url: &str, rev: Option<&str>) -> Result<String> {
    if let Some(rev) = rev
        && is_full_object_id(rev)
    {
        return Ok(rev.to_string());
    }

    let name = rev.unwrap_or("HEAD");
    let output = git::git_cmd()?
        .arg("ls-remote")
        .arg(url)
        .arg(name)
        .output()
        .await?
        .stdout;

    match (select_remote_ref(str::from_utf8(&output)?, name), rev) {
        (Some(object_id), _) => Ok(object_id.to_string()),
        (None, Some(rev)) => Ok(rev.to_string()),
        (None, None) => anyhow::bail!("Failed to resolve the HEAD revision of `{url}`"),
    }
}

/// Pick the object ID of the ref that `git fetch origin <name>` would fetch from `git ls-remote
/// <url> <name>` output.
///
/// `ls-remote` lists every ref whose name ends with `name`, so the candidates are checked in the
/// order Git uses to resolve a short ref name.
fn select_remote_ref<'a>(ls_remote_output: &'a str, name: &str) -> Option<&'a str> {
    let refs: Vec<(&str, &str)> = ls_remote_output
        .lines()
        .filter_map(|line| line.split_once('\t'))
        .collect();
    let candidates = [
        name.to_string(),
        format!("refs/{name}"),
        format!("refs/tags/{name}"),
        format!("refs/heads/{name}"),
        format!("refs/remotes/{name}"),
    ];
    candidates.iter().find_map(|candidate| {
        refs.iter()
            .find(|(_, refname)| refname == candidate)
            .map(|(object_id, _)| *object_id)
    })
}

/// Create an empty directory for the shadow copy of `repo_path`.
///
/// The directory name depends only on the source repository, so hook environments installed
/// from the shadow copy keep the same repository identity across runs and can be reused.
/// Callers must hold the store lock: it keeps concurrent `try-repo` runs for the same source from
/// sharing the directory and keeps cache GC from removing it while it is in use.
fn create_shadow_dir(scratch: &Path, repo_path: &Path) -> Result<TempDir> {
    let mut hasher = SeaHasher::new();
    repo_path.hash(&mut hasher);
    let name = format!(
        "try-repo-shadow-{}",
        hex::encode(hasher.finish().to_le_bytes())
    );

    // A previous run that was killed may have left the directory behind.
    match fs_err::remove_dir_all(scratch.join(&name)) {
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => return Err(err.into()),
        _ => {}
    }

    Ok(tempfile::Builder::new()
        .prefix(&name)
        .rand_bytes(0)
        .tempdir_in(scratch)?)
}

async fn clone_and_commit(repo_path: &Path, head_rev: &str, shadow: &Path) -> Result<()> {
    git::git_cmd()?
        .arg("clone")
        .arg(repo_path)
        .arg(shadow)
        .output()
        .await?;
    git::git_cmd()?
        .arg("checkout")
        .arg(head_rev)
        .arg("-b")
        .arg("_prek_tmp")
        .current_dir(shadow)
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
        .env("GIT_INDEX_FILE", &index_path)
        .env("GIT_OBJECT_DIRECTORY", &objects_path)
        .output()
        .await?;

    // Fixed identity and dates make the commit hash depend only on the parent and the tree, so
    // unchanged contents produce the same revision and reuse the installed hook environments.
    git::git_cmd()?
        .arg("commit")
        .arg("-m")
        .arg("Temporary commit by prek try-repo")
        .arg("--no-gpg-sign")
        .arg("--no-edit")
        .arg("--no-verify")
        .current_dir(shadow)
        .env("GIT_AUTHOR_NAME", "prek test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_AUTHOR_DATE", SHADOW_COMMIT_DATE)
        .env("GIT_COMMITTER_NAME", "prek test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_DATE", SHADOW_COMMIT_DATE)
        .output()
        .await?;

    Ok(())
}

const SHADOW_COMMIT_DATE: &str = "2000-01-01T00:00:00+00:00";

struct PreparedRepo<'a> {
    runtime_source: Cow<'a, str>,
    display_source: Option<&'a str>,
    rev: String,
    /// The shadow copy of a local repository with uncommitted changes. It is only needed until
    /// the store has cloned it, and dropping it removes the directory.
    _shadow: Option<TempDir>,
}

async fn prepare_repo<'a>(
    repo: &'a str,
    rev: Option<&str>,
    scratch: &Path,
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

    // The store caches clones and hook environments by revision, so resolve refs such as
    // branches to what they point to now. Otherwise a later run would reuse an outdated clone.
    let resolved_rev = if is_local {
        resolve_local_rev(repo_path, rev.unwrap_or("HEAD")).await?
    } else {
        resolve_remote_rev(repo, rev).await?
    };

    // Uncommitted changes only apply when trying the checked-out `HEAD`.
    if rev.is_none() && is_local && git::has_diff("HEAD", repo_path).await? {
        warn_user!("Local repository has uncommitted changes. Creating a temporary copy...");
        let shadow = create_shadow_dir(scratch, repo_path)?;
        clone_and_commit(repo_path, &resolved_rev, shadow.path()).await?;
        let head_rev = resolve_local_rev(shadow.path(), "HEAD").await?;
        Ok(PreparedRepo {
            runtime_source: Cow::Owned(shadow.path().to_string_lossy().into_owned()),
            display_source: None,
            rev: head_rev,
            _shadow: Some(shadow),
        })
    } else {
        Ok(PreparedRepo {
            runtime_source,
            display_source: is_local.then_some(repo),
            rev: resolved_rev,
            _shadow: None,
        })
    }
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

    // Use the shared store so repeated runs reuse cloned revisions, hook environments, and
    // toolchains. Only the generated config is temporary.
    let store = Store::from_settings()?;
    let selectors = Selectors::load(&run_args.includes, &run_args.skips, git::root()?)?;

    let predefined_hooks = match repo.as_str() {
        "builtin" => Some(select_predefined_hooks::<BuiltinHooks>(&selectors)),
        "meta" => Some(select_predefined_hooks::<MetaHooks>(&selectors)),
        _ => None,
    };

    // Cache GC removes scratch, and any clone or hook environment that no tracked config uses.
    // Hold the store lock until the generated config is tracked so GC cannot remove what this run
    // is about to use. `run` takes the lock again, so it must be released before running hooks.
    let lock = store.lock_async().await?;
    let tmp_dir = TempDir::with_prefix_in("try-repo-", store.scratch_path())?;
    let config_file = tmp_dir.path().join(PREK_TOML);

    let (config_str, display_config_str) = if let Some(hooks) = predefined_hooks {
        if rev.is_some() {
            warn_user!("`--rev` option is ignored for the `{repo}` repo");
        }
        (render_repo_config_toml(&repo, None, &hooks), None)
    } else {
        let prepared = prepare_repo(&repo, rev.as_deref(), &store.scratch_path())
            .await
            .context("Failed to determine repository and revision")?;

        let repo_config = config::RemoteRepo::new(
            prepared.runtime_source.to_string(),
            prepared.rev.clone(),
            vec![],
        );
        let repo_clone_path = store.clone_repo(&repo_config, None).await?;
        let manifest =
            config::read_manifest(&repo_clone_path.join(prek_consts::PRE_COMMIT_HOOKS_YAML))?;
        let hooks = manifest
            .hooks
            .into_iter()
            .filter(|hook| selectors.matches_hook_id(&hook.id))
            .map(|hook| hook.id)
            .collect::<Vec<_>>();

        // The scratch config needs the resolved source and revision, while the displayed config
        // should preserve the user's path and `--ref` so it remains meaningful when copied into
        // their project.
        let config_str =
            render_repo_config_toml(&prepared.runtime_source, Some(&prepared.rev), &hooks);
        let display_source = prepared.display_source.unwrap_or(&prepared.runtime_source);
        let display_rev = rev.as_deref().unwrap_or(&prepared.rev);
        let display_config_str = render_repo_config_toml(display_source, Some(display_rev), &hooks);
        (config_str, Some(display_config_str))
    };
    fs_err::tokio::write(&config_file, &config_str).await?;
    store.track_configs(std::iter::once(config_file.as_path()))?;
    drop(lock);

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
        &store,
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

#[cfg(test)]
mod tests {
    use super::{is_full_object_id, select_remote_ref};

    const LS_REMOTE: &str = "\
1111111111111111111111111111111111111111\tHEAD
2222222222222222222222222222222222222222\trefs/heads/feature/main
3333333333333333333333333333333333333333\trefs/heads/main
4444444444444444444444444444444444444444\trefs/tags/main
5555555555555555555555555555555555555555\trefs/tags/main^{}
";

    #[test]
    fn select_remote_ref_follows_git_short_name_order() {
        assert_eq!(
            select_remote_ref(LS_REMOTE, "HEAD"),
            Some("1111111111111111111111111111111111111111")
        );
        // A tag wins over a branch with the same name, as it does for `git fetch`.
        assert_eq!(
            select_remote_ref(LS_REMOTE, "main"),
            Some("4444444444444444444444444444444444444444")
        );
        assert_eq!(
            select_remote_ref(LS_REMOTE, "heads/main"),
            Some("3333333333333333333333333333333333333333")
        );
        assert_eq!(
            select_remote_ref(LS_REMOTE, "refs/heads/feature/main"),
            Some("2222222222222222222222222222222222222222")
        );
    }

    #[test]
    fn select_remote_ref_ignores_partial_matches() {
        // `ls-remote` matches `main` against `refs/heads/feature/main`, but `git fetch` does not.
        let output = "2222222222222222222222222222222222222222\trefs/heads/feature/main\n";
        assert_eq!(select_remote_ref(output, "main"), None);
        assert_eq!(select_remote_ref("", "main"), None);
    }

    #[test]
    fn full_object_ids() {
        assert!(is_full_object_id(&"a".repeat(40)));
        assert!(is_full_object_id(&"0".repeat(64)));
        assert!(!is_full_object_id("abc1234"));
        assert!(!is_full_object_id(&"g".repeat(40)));
    }
}
