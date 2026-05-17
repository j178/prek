use std::borrow::Cow;
use std::fmt::Write;
use std::fs::Metadata;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result};
use gix::bstr::{BStr, BString, ByteSlice as _};
use owo_colors::OwoColorize;
use prek_consts::PREK_TOML;
use tempfile::TempDir;
use toml_edit::{Array, ArrayOfTables, DocumentMut, InlineTable, Item, Value};

use crate::cli::run::Selectors;
use crate::cli::{ExitStatus, flag};
use crate::config;
use crate::git;
use crate::git::GIT_ROOT;
use crate::printer::Printer;
use crate::store::Store;
use crate::warn_user;

fn repo_head_commit(repo: &Path) -> Result<String> {
    Ok(git::head_commit(repo)?)
}

fn git_path(path: &Path) -> BString {
    let path = gix::path::into_bstr(path);
    gix::path::to_unix_separators_on_windows(path.as_ref()).into_owned()
}

#[cfg(unix)]
fn is_executable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable(_metadata: &Metadata) -> bool {
    false
}

fn worktree_blob(
    path: &Path,
    existing_mode: Option<gix::index::entry::Mode>,
    tracks_file_mode: bool,
) -> Result<Option<(Vec<u8>, gix::index::entry::Mode)>> {
    let metadata = match fs_err::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };

    if metadata.file_type().is_symlink() {
        return Ok(Some((
            git_path(&fs_err::read_link(path)?).into(),
            gix::index::entry::Mode::SYMLINK,
        )));
    }

    if !metadata.is_file() {
        return Ok(None);
    }

    let mode = if tracks_file_mode {
        if is_executable(&metadata) {
            gix::index::entry::Mode::FILE_EXECUTABLE
        } else {
            gix::index::entry::Mode::FILE
        }
    } else {
        existing_mode.unwrap_or(gix::index::entry::Mode::FILE)
    };

    Ok(Some((fs_err::read(path)?, mode)))
}

fn sync_worktree_path_to_index(
    repo: &gix::Repository,
    index: &mut gix::index::File,
    source_root: &Path,
    path: &BStr,
    tracks_file_mode: bool,
) -> Result<()> {
    let entry_index =
        index.entry_index_by_path_and_stage(path, gix::index::entry::Stage::Unconflicted);
    let existing_mode = entry_index.map(|idx| index.entries()[idx].mode);
    let source_path = source_root.join(gix::path::from_bstr(path));

    let Some((content, mode)) = worktree_blob(&source_path, existing_mode, tracks_file_mode)?
    else {
        if let Some(idx) = entry_index {
            index.remove_entry_at_index(idx);
            index.remove_tree();
        }
        return Ok(());
    };

    let id = repo.write_blob(content)?.detach();
    if let Some(idx) = entry_index {
        let entry = &mut index.entries_mut()[idx];
        entry.stat = gix::index::entry::Stat::default();
        entry.id = id;
        entry.flags = gix::index::entry::Flags::empty();
        entry.mode = mode;
    } else {
        index.dangerously_push_entry(
            gix::index::entry::Stat::default(),
            id,
            gix::index::entry::Flags::empty(),
            mode,
            path,
        );
        index.sort_entries();
    }
    index.remove_tree();

    Ok(())
}

fn clone_and_commit(repo_path: &Path, _head_rev: &str, tmp_dir: &Path) -> Result<PathBuf> {
    let shadow = tmp_dir.join("shadow-repo");
    let should_interrupt = AtomicBool::new(false);
    let mut clone = gix::prepare_clone(repo_path.to_string_lossy().as_ref(), &shadow)?;
    let (mut checkout, _) = clone.fetch_then_checkout(gix::progress::Discard, &should_interrupt)?;
    checkout.main_worktree(gix::progress::Discard, &should_interrupt)?;

    let shadow_repo = gix::open(&shadow)?;
    let mut shadow_index = shadow_repo.open_index()?;
    let tracks_file_mode = gix::open(repo_path)?
        .config_snapshot()
        .boolean("core.fileMode")
        .unwrap_or(true);
    let tracked_paths = shadow_index
        .entries()
        .iter()
        .filter(|entry| entry.stage_raw() == 0)
        .map(|entry| entry.path(&shadow_index).to_owned())
        .collect::<Vec<_>>();
    for path in &tracked_paths {
        sync_worktree_path_to_index(
            &shadow_repo,
            &mut shadow_index,
            repo_path,
            path.as_bstr(),
            tracks_file_mode,
        )?;
    }

    let staged_files = git::get_staged_files(repo_path)?;
    for path in &staged_files {
        let path = git_path(path);
        sync_worktree_path_to_index(
            &shadow_repo,
            &mut shadow_index,
            repo_path,
            path.as_bstr(),
            tracks_file_mode,
        )?;
    }

    shadow_index.sort_entries();
    shadow_index.write(gix::index::write::Options::default())?;
    let tree = git::write_index_tree(&shadow_repo, &shadow_index)?;
    let parent = shadow_repo.head_id()?.detach();
    let signature = gix::actor::Signature {
        name: "prek test".into(),
        email: "test@example.com".into(),
        time: gix::date::Time::now_local_or_utc(),
    };
    let mut committer_time = gix::date::parse::TimeBuf::default();
    let mut author_time = gix::date::parse::TimeBuf::default();
    shadow_repo.commit_as(
        signature.to_ref(&mut committer_time),
        signature.to_ref(&mut author_time),
        "HEAD",
        "Temporary commit by prek try-repo",
        tree,
        [parent],
    )?;

    Ok(shadow)
}

fn prepare_repo_and_rev<'a>(
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
        repo_head_commit(repo_path)?
    } else {
        git::remote_head_commit(repo)?
    };

    // If repo is a local repo with uncommitted changes, create a shadow repo to commit the changes.
    if is_local && git::has_diff("HEAD", repo_path)? {
        warn_user!("Creating temporary repo with uncommitted changes...");
        let shadow = clone_and_commit(repo_path, &head_rev, tmp_dir)?;
        let head_rev = repo_head_commit(&shadow)?;
        Ok((Cow::Owned(shadow.to_string_lossy().to_string()), head_rev))
    } else {
        Ok((Cow::Borrowed(repo), head_rev))
    }
}

fn render_repo_config_toml(repo_path: &str, rev: &str, hooks: Vec<String>) -> String {
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
        .context("Failed to determine repository and revision")?;

    let store = Store::from_path(tmp_dir.path()).init()?;
    let repo_config = config::RemoteRepo::new(repo_path.to_string(), rev.clone(), vec![]);
    let repo_clone_path = store.clone_remote_repo(&repo_config, None).await?;

    let selectors = Selectors::load(&run_args.includes, &run_args.skips, GIT_ROOT.as_ref()?)?;

    let manifest =
        config::read_manifest(&repo_clone_path.join(prek_consts::PRE_COMMIT_HOOKS_YAML))?;

    let hooks = manifest
        .hooks
        .into_iter()
        .filter(|hook| selectors.matches_hook_id(&hook.id))
        .map(|hook| hook.id)
        .collect::<Vec<_>>();

    let config_str = render_repo_config_toml(&repo_path, &rev, hooks);
    let config_file = tmp_dir.path().join(PREK_TOML);
    fs_err::tokio::write(&config_file, &config_str).await?;

    writeln!(
        printer.stdout(),
        "{}",
        format!("Using generated `{PREK_TOML}`:").cyan().bold()
    )?;
    writeln!(printer.stdout(), "{}", config_str.dimmed())?;

    crate::cli::run(
        &store,
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
        flag(run_args.fail_fast, run_args.no_fail_fast),
        run_args.dry_run,
        refresh,
        run_args.extra,
        verbose,
        printer,
    )
    .await
}
