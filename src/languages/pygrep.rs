use std::fmt::Write;
use std::path::Path;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;
use fancy_regex::Regex;
use futures::StreamExt;
use tokio::io::AsyncBufReadExt;

use crate::hook::{Hook, InstalledHook};
use crate::languages::LanguageImpl;
use crate::run::CONCURRENCY;
use crate::store::Store;

#[derive(Parser)]
struct Args {
    #[arg(short, long)]
    ignore_case: bool,
    #[arg(long)]
    multiline: bool,
    #[arg(long)]
    negate: bool,
}

pub(crate) struct Pygrep;

impl LanguageImpl for Pygrep {
    async fn install(&self, hook: Arc<Hook>, _store: &Store) -> Result<InstalledHook> {
        Ok(InstalledHook::NoNeedInstall(hook))
    }

    async fn check_health(&self) -> Result<()> {
        todo!()
    }

    async fn run(
        &self,
        hook: &InstalledHook,
        filenames: &[&String],
        _store: &Store,
    ) -> Result<(i32, Vec<u8>)> {
        let args = Args::try_parse_from(&hook.args).context("Failed to parse `args`")?;
        // For `pygrep`, its entry is a Python regex pattern.
        let pattern = if args.ignore_case {
            Regex::new(&format!("(?i){}", hook.entry.entry()))
        } else {
            Regex::new(hook.entry.entry())
        }
        .context("Failed to parse `entry` as regex")?;

        let mut tasks = futures::stream::iter(filenames.iter())
            .map(async |filename| {
                let filename = Path::new(filename);
                match (args.multiline, args.negate) {
                    (true, true) => process_file_at_once_negated(filename, &pattern).await,
                    (true, false) => process_file_at_once(filename, &pattern).await,
                    (false, true) => process_file_by_line_negated(filename, &pattern).await,
                    (false, false) => process_file_by_line(filename, &pattern).await,
                }
            })
            .buffered(*CONCURRENCY);

        let mut code = 0;
        let mut output = Vec::new();

        while let Some(Ok(result)) = tasks.next().await {
            if !result.is_empty() {
                code = 1;
                output.extend(result.into_bytes());
            }
        }

        Ok((code, output))
    }
}

async fn process_file_by_line(filename: &Path, pattern: &Regex) -> Result<String> {
    let file = fs_err::tokio::File::open(filename).await?;
    let mut reader = tokio::io::BufReader::new(file);
    let mut output = String::new();
    let mut line = String::new();

    let mut line_no = 1;
    while reader.read_line(&mut line).await? > 0 {
        if pattern.is_match(&line)? {
            writeln!(
                &mut output,
                "{}:{}:{}",
                filename.display(),
                line_no,
                line.trim_end()
            )?;
        }
        line_no += 1;
        line.clear();
    }

    Ok(output)
}

async fn process_file_at_once(filename: &Path, pattern: &Regex) -> Result<String> {
    let content = fs_err::tokio::read_to_string(filename).await?;
    if let Some(m) = pattern.find(&content)? {
        let line_no = content[..m.start()].lines().count() + 1;
        let mut output = String::new();
        writeln!(&mut output, "{}:{}:", filename.display(), line_no)?;

        let mut matched = m.as_str().split('\n').collect::<Vec<_>>();
        if let Some(line) = content.lines().nth(line_no) {
            matched[0] = line;
        }
        for line in matched {
            writeln!(&mut output, "{}", line.trim_end())?;
        }

        Ok(output)
    } else {
        Ok(String::new())
    }
}

async fn process_file_by_line_negated(filename: &Path, pattern: &Regex) -> Result<String> {
    let file = fs_err::tokio::File::open(filename).await?;
    let mut reader = tokio::io::BufReader::new(file);
    let mut output = String::new();
    let mut line = String::new();

    let mut line_no = 1;
    while reader.read_line(&mut line).await? > 0 {
        if !pattern.is_match(&line)? {
            writeln!(
                &mut output,
                "{}:{}:{}",
                filename.display(),
                line_no,
                line.trim_end()
            )?;
            return Ok(output);
        }
        line_no += 1;
        line.clear();
    }

    Ok(output)
}

async fn process_file_at_once_negated(filename: &Path, pattern: &Regex) -> Result<String> {
    let content = fs_err::tokio::read_to_string(filename).await?;
    if !pattern.is_match(&content)? {
        return Ok(filename.to_string_lossy().to_string());
    }

    Ok(String::new())
}
