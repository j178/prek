use std::path::Path;

use anyhow::Result;
use bstr::ByteSlice;
use clap::Parser;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

#[derive(Parser)]
struct Args {
    #[arg(long)]
    markdown_linebreak_ext: Vec<String>,
    #[arg(long)]
    chars: Vec<char>,
}

pub(crate) async fn fix_trailing_whitespace(
    hook: &Hook,
    filenames: &[&String],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.parsed()?.iter().chain(&hook.args))?;

    let force_markdown = args.markdown_linebreak_ext.iter().any(|ext| ext == "*");
    let markdown_exts = args
        .markdown_linebreak_ext
        .iter()
        .flat_map(|ext| ext.split(','))
        .map(|ext| format!(".{}", ext.trim_start_matches('.')).to_ascii_lowercase())
        .collect::<Vec<_>>();
    let chars = if args.chars.is_empty() {
        None
    } else {
        Some(args.chars)
    };

    // Validate extensions don't contain path separators
    for ext in &markdown_exts {
        if ext[1..]
            .chars()
            .any(|c| matches!(c, '.' | '/' | '\\' | ':'))
        {
            return Err(anyhow::anyhow!(
                "bad --markdown-linebreak-ext extension '{ext}' (has . / \\ :)"
            ));
        }
    }

    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| {
            let ext = Path::new(filename)
                .extension()
                .and_then(|ext| ext.to_str())
                .map(|ext| format!(".{}", ext.to_ascii_lowercase()));
            let is_markdown = force_markdown || ext.is_some_and(|ext| markdown_exts.contains(&ext));

            // TODO: read file in chunks
            let content = fs_err::tokio::read(filename).await?;

            let mut modified = false;
            let mut output = Vec::new();

            for mut line in content.split_inclusive(|&b| b == b'\n') {
                let eol = if line.ends_with(b"\r\n") {
                    line = &line[..line.len() - 2];
                    b"\r\n".as_slice()
                } else if line.ends_with(b"\n") {
                    line = &line[..line.len() - 1];
                    b"\n".as_slice()
                } else {
                    b"".as_slice()
                };

                if line.is_empty() {
                    output.extend_from_slice(eol);
                    continue;
                }

                let output_len = output.len();

                if is_markdown
                    && !line.iter().all(|&b| b.is_ascii_whitespace())
                    && line.ends_with(b"  ")
                {
                    // Preserve trailing two spaces for markdown, but trim any additional whitespace
                    let trimmed = if let Some(chars) = chars.as_deref() {
                        line[..line.len() - 2].trim_end_with(|b| chars.contains(&b))
                    } else {
                        line[..line.len() - 2].trim_ascii_end()
                    };
                    output.extend_from_slice(trimmed);
                    output.extend_from_slice(b"  ");
                    output.extend_from_slice(eol);
                } else {
                    // Normal whitespace trimming
                    let trimmed = if let Some(chars) = chars.as_deref() {
                        line.trim_end_with(|b| chars.contains(&b))
                    } else {
                        line.trim_ascii_end()
                    };
                    output.extend_from_slice(trimmed);
                    output.extend_from_slice(eol);
                }

                if line.len() + eol.len() != output.len() - output_len {
                    modified = true;
                }
            }

            if modified {
                fs_err::tokio::write(filename, &output).await?;
                anyhow::Ok((1, format!("Fixing {filename}\n").into_bytes()))
            } else {
                anyhow::Ok((0, Vec::new()))
            }
        })
        .buffered(*CONCURRENCY);

    let mut code = 0;
    let mut output = Vec::new();

    while let Some(result) = tasks.next().await {
        let (c, o) = result?;
        code |= c;
        output.extend(o);
    }

    Ok((code, output))
}
