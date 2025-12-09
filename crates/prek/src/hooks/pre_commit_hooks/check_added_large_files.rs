use std::path::{Path, PathBuf};

use clap::Parser;
use futures::StreamExt;
use rustc_hash::FxHashSet;

use crate::git::{get_lfs_files, get_staged_files};
use crate::hook::Hook;
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
        let add_files = get_staged_files(hook.work_dir())
            .await?
            .into_iter()
            .collect::<FxHashSet<_>>();
        FileFilter::Files(add_files)
    };

    let lfs_files = get_lfs_files(filenames).await?;

    let mut tasks = futures::stream::iter(
        filenames
            .iter()
            .filter(|f| filter.contains(f))
            .filter(|f| !lfs_files.contains(**f)),
    )
    .map(async |filename| {
        let file_path = hook.project().relative_path().join(filename);
        let size = fs_err::tokio::metadata(file_path).await?.len();
        let size = size / 1024;
        if size > args.max_kb {
            anyhow::Ok(Some(format!(
                "{} ({size} KB) exceeds {} KB\n",
                filename.display(),
                args.max_kb
            )))
        } else {
            anyhow::Ok(None)
        }
    })
    .buffered(*CONCURRENCY);

    let mut code = 0;
    let mut output = Vec::new();

    while let Some(result) = tasks.next().await {
        if let Some(e) = result? {
            code = 1;
            output.extend(e.into_bytes());
        }
    }

    Ok((code, output))
}
