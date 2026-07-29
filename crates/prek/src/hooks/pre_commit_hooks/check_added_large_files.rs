use std::path::{Path, PathBuf};

use clap::Parser;
use rustc_hash::FxHashSet;

use crate::git::{lfs_files, staged_added_files};
use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{hook_filenames, parse_hook_args};
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Check all files, not just those staged for addition.
    #[arg(long)]
    enforce_all: bool,
    /// Maximum allowed file size in KiB.
    #[arg(long = "maxkb", default_value = "500")]
    max_kb: u64,
    #[arg(value_name = "FILENAMES")]
    filenames: Vec<PathBuf>,
}

/// Runs the `check-added-large-files` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> anyhow::Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;
    let all_filenames = hook_filenames(&args.filenames, filenames).collect::<Vec<_>>();
    let filenames = all_filenames.as_slice();

    let candidate_filenames;
    let filenames = if args.enforce_all {
        filenames
    } else {
        let added_files = staged_added_files(hook.work_dir())
            .await?
            .into_iter()
            .collect::<FxHashSet<_>>();
        candidate_filenames = filenames
            .iter()
            .copied()
            .filter(|filename| added_files.contains(*filename))
            .collect::<Vec<_>>();
        candidate_filenames.as_slice()
    };

    if filenames.is_empty() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    // Builtin hooks receive project-relative filenames, so git attribute lookups need to run
    // from the project root for nested `.gitattributes` files to apply.
    let lfs_files = lfs_files(hook.work_dir(), filenames).await?;

    let filenames = filenames
        .iter()
        .copied()
        .filter(|f| !lfs_files.contains(*f));

    run_concurrent_file_checks(filenames, *INTERNAL_CONCURRENCY, |filename| async move {
        let file_path = hook.project().relative_path().join(filename);
        let size = fs_err::tokio::metadata(file_path).await?.len() / 1024;
        if size > args.max_kb {
            anyhow::Ok(HookOutput::unchanged(
                1,
                format!(
                    "{} ({size} KB) exceeds {} KB\n",
                    filename.display(),
                    args.max_kb
                )
                .into_bytes(),
            ))
        } else {
            anyhow::Ok(HookOutput::unchanged(0, Vec::new()))
        }
    })
    .await
}
