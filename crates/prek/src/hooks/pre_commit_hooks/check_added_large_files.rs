use std::fmt::Write;
use std::path::{Path, PathBuf};

use clap::Parser;
use rayon::iter::{IntoParallelIterator, ParallelIterator};
use rustc_hash::FxHashSet;

use crate::git::{lfs_files, staged_added_files};
use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{hook_filenames, parse_hook_args};

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
    let filenames = hook_filenames(&args.filenames, filenames)
        .map(Path::to_path_buf)
        .collect::<Vec<_>>();
    let file_base = hook.project().relative_path().to_path_buf();
    let mut candidates = tokio::task::spawn_blocking(move || {
        filenames
            .into_par_iter()
            .filter_map(|filename| {
                let size = fs_err::metadata(file_base.join(&filename))
                    .map(|metadata| metadata.len() / 1024);
                if let Ok(size) = size
                    && size <= args.max_kb
                {
                    None
                } else {
                    Some((filename, size))
                }
            })
            .collect::<Vec<_>>()
    })
    .await?;

    if candidates.is_empty() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    if !args.enforce_all {
        let added_files = staged_added_files(hook.work_dir())
            .await?
            .into_iter()
            .collect::<FxHashSet<_>>();
        candidates.retain(|(filename, _)| added_files.contains(filename));
    }

    let filenames = candidates
        .iter()
        .map(|(filename, _)| filename.as_path())
        .collect::<Vec<_>>();
    // Filenames are project-relative, including for nested `.gitattributes` lookups.
    let lfs_files = lfs_files(hook.work_dir(), &filenames).await?;

    let mut output = String::new();
    for (filename, size) in candidates {
        if lfs_files.contains(&filename) {
            continue;
        }
        let size = size?;
        writeln!(
            output,
            "{} ({size} KB) exceeds {} KB",
            filename.display(),
            args.max_kb
        )?;
    }
    let exit_code = i32::from(!output.is_empty());
    Ok(HookOutput::unchanged(exit_code, output.into_bytes()))
}
