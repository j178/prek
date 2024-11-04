use std::fmt::Write;
use std::path::{Path, PathBuf};

use anyhow::Result;
use futures::stream::FuturesUnordered;
use futures::StreamExt;
use itertools::Itertools;
use owo_colors::OwoColorize;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use tokio::process::Command;
use tracing::{debug, trace};

use crate::cli::ExitStatus;
use crate::config::Stage;
use crate::fs::{normalize_path, Simplified};
use crate::git::{get_all_files, get_changed_files, get_staged_files, GIT};
use crate::hook::{Hook, Project};
use crate::printer::Printer;
use crate::run::{run_hooks, FilenameFilter};
use crate::store::Store;

#[allow(clippy::too_many_arguments)]
pub(crate) async fn run(
    config: Option<PathBuf>,
    hook_id: Option<String>,
    hook_stage: Option<Stage>,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<PathBuf>,
    show_diff_on_failure: bool,
    verbose: bool,
    printer: Printer,
) -> Result<ExitStatus> {
    let config_file = Project::find_config_file(config)?;
    if config_not_staged(&config_file).await? {
        writeln!(
            printer.stderr(),
            "Your pre-commit configuration is unstaged.\n`git add {}` to fix this.",
            &config_file.user_display()
        )?;
        return Ok(ExitStatus::Failure);
    }

    let mut project = Project::new(config_file)?;
    let store = Store::from_settings()?.init()?;

    // TODO: check .pre-commit-config.yaml status and git status
    // TODO: fill env vars
    // TODO: impl staged_files_only

    let lock = store.lock_async().await?;
    let hooks = project.init_hooks(&store, printer).await?;

    let hooks: Vec<_> = hooks
        .into_iter()
        .filter(|h| {
            if let Some(ref hook) = hook_id {
                &h.id == hook || &h.alias == hook
            } else {
                true
            }
        })
        .filter(|h| {
            if let Some(stage) = hook_stage {
                h.stages.contains(&stage)
            } else {
                true
            }
        })
        .collect();

    if hooks.is_empty() && hook_id.is_some() {
        if let Some(hook_stage) = hook_stage {
            writeln!(
                printer.stderr(),
                "No hook found for id `{}` and stage `{}`",
                hook_id.unwrap().cyan(),
                hook_stage.cyan()
            )?;
        } else {
            writeln!(
                printer.stderr(),
                "No hook found for id `{}`",
                hook_id.unwrap().cyan()
            )?;
        }
        return Ok(ExitStatus::Failure);
    }

    let skips = get_skips();
    let to_install = hooks
        .iter()
        .filter(|h| !skips.contains(&h.id) && !skips.contains(&h.alias))
        .cloned()
        .collect::<Vec<_>>();

    debug!(
        "Hooks going to run: {:?}",
        to_install.iter().map(|h| &h.id).collect::<Vec<_>>()
    );
    install_hooks(&to_install, printer).await?;
    drop(lock);

    let mut filenames = all_filenames(hook_stage, from_ref, to_ref, all_files, files).await?;
    for filename in &mut filenames {
        normalize_path(filename);
    }

    let filter = FilenameFilter::new(
        project.config().files.as_deref(),
        project.config().exclude.as_deref(),
    )?;
    let filenames = filenames
        .into_par_iter()
        .filter(|filename| filter.filter(filename))
        .filter(|filename| {
            // Ignore not existing files.
            std::fs::symlink_metadata(filename)
                .map(|m| m.file_type().is_file())
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();

    trace!("Files after filtered: {}", filenames.len());

    run_hooks(
        &hooks,
        &skips,
        filenames,
        project.config().fail_fast.unwrap_or(false),
        show_diff_on_failure,
        verbose,
        printer,
    )
    .await?;

    Ok(ExitStatus::Success)
}

async fn config_not_staged(config: &Path) -> Result<bool> {
    let output = Command::new(GIT.as_ref()?)
        .arg("diff")
        .arg("--quiet") // Implies --exit-code
        .arg("--no-ext-diff")
        .arg(config)
        .status()
        .await?;

    Ok(!output.success())
}

fn get_skips() -> Vec<String> {
    match std::env::var_os("SKIP") {
        Some(s) if !s.is_empty() => s
            .to_string_lossy()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>(),
        _ => vec![],
    }
}

/// Get all filenames to run hooks on.
#[allow(clippy::too_many_arguments)]
async fn all_filenames(
    hook_stage: Option<Stage>,
    from_ref: Option<String>,
    to_ref: Option<String>,
    all_files: bool,
    files: Vec<PathBuf>,
) -> Result<Vec<String>> {
    if hook_stage.is_some_and(|stage| !stage.operate_on_files()) {
        return Ok(vec![]);
    }
    // if hook_stage.is_some_and(|stage| matches!(stage, Stage::PrepareCommitMsg | Stage::CommitMsg)) {
    //     return iter::once(commit_msg_filename.unwrap());
    // }
    if let (Some(from_ref), Some(to_ref)) = (from_ref, to_ref) {
        let files = get_changed_files(&from_ref, &to_ref).await?;
        debug!(
            "Files changed between {} and {}: {}",
            from_ref,
            to_ref,
            files.len()
        );
        return Ok(files);
    }

    if !files.is_empty() {
        let files: Vec<_> = files
            .into_iter()
            .map(|f| f.to_string_lossy().to_string())
            .collect();
        debug!("Files passed as arguments: {}", files.len());
        return Ok(files);
    }
    if all_files {
        let files = get_all_files().await?;
        debug!("All files in the repo: {}", files.len());
        return Ok(files);
    }
    // if is_in_merge_conflict() {
    //     return get_conflicted_files();
    // }
    let files = get_staged_files().await?;
    debug!("Staged files: {}", files.len());
    Ok(files)
}

async fn install_hook(hook: &Hook, env_dir: PathBuf, printer: Printer) -> Result<()> {
    writeln!(
        printer.stdout(),
        "Installing environment for {}",
        hook.repo(),
    )?;
    debug!("Install environment for {} to {}", hook, env_dir.display());

    if env_dir.try_exists()? {
        debug!(
            "Removing existing environment directory {}",
            env_dir.display()
        );
        fs_err::remove_dir_all(&env_dir)?;
    }

    hook.language.install(hook).await?;
    hook.mark_installed()?;

    Ok(())
}

// TODO: progress bar
async fn install_hooks(hooks: &[Hook], printer: Printer) -> Result<()> {
    let to_install = hooks
        .iter()
        .filter(|&hook| !hook.installed())
        .unique_by(|&hook| hook.install_key());

    let mut tasks = FuturesUnordered::new();
    for hook in to_install {
        if let Some(env_dir) = hook.environment_dir() {
            tasks.push(async move { install_hook(hook, env_dir, printer).await });
        }
    }
    while let Some(result) = tasks.next().await {
        result?;
    }

    Ok(())
}
