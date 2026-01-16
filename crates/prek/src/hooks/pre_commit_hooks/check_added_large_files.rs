use std::path::{Path, PathBuf};

use clap::Parser;
use rustc_hash::FxHashSet;

use crate::git::{get_added_files, get_lfs_files};
use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::run::CONCURRENCY;

enum FileFilter {
    NoFilter,
    Files(FxHashSet<PathBuf>),
}

impl FileFilter {
    fn contains(&self, path: &Path) -> bool {
        match self {
            FileFilter::NoFilter => true,
            FileFilter::Files(files) => files.contains(path),
        }
    }
}

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long)]
    enforce_all: bool,
    #[arg(long = "maxkb", default_value = "500")]
    max_kb: u64,
}

pub(crate) async fn check_added_large_files(
    hook: &Hook,
    filenames: &[&Path],
) -> anyhow::Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    let filter = if args.enforce_all {
        FileFilter::NoFilter
    } else {
        let add_files = get_added_files(hook.work_dir())
            .await?
            .into_iter()
            .collect::<FxHashSet<_>>();
        FileFilter::Files(add_files)
    };

    let lfs_files = get_lfs_files(filenames).await?;

    let filenames = filenames
        .iter()
        .copied()
        .filter(|f| filter.contains(f))
        .filter(|f| !lfs_files.contains(*f));

    run_concurrent_file_checks(filenames, *CONCURRENCY, |filename| async move {
        let file_path = hook.project().relative_path().join(filename);
        let size = fs_err::tokio::metadata(file_path).await?.len() / 1024;
        if size > args.max_kb {
            anyhow::Ok((
                1,
                format!(
                    "{} ({size} KB) exceeds {} KB\n",
                    filename.display(),
                    args.max_kb
                )
                .into_bytes(),
            ))
        } else {
            anyhow::Ok((0, Vec::new()))
        }
    })
    .await
}
