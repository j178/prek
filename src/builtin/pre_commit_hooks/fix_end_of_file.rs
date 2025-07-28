use std::collections::HashMap;

use anyhow::Result;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

pub(crate) async fn fix_end_of_file(
    _hook: &Hook,
    filenames: &[&String],
    _env_vars: &HashMap<&'static str, String>,
) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| {
            // TODO: avoid reading the whole file into memory
            let content = fs_err::tokio::read(filename).await?;

            // If the file is empty, do nothing.
            if content.is_empty() {
                return Ok((0, Vec::new()));
            }

            // Find the last character that is not a newline char.
            let last_char_pos = content.iter().rposition(|&c| c != b'\n' && c != b'\r');

            // FIXME: /r/n should be kept as is
            if let Some(pos) = last_char_pos {
                // The file has content other than newlines.
                let new_content = [&content[..=pos], b"\n"].concat();
                if new_content == content {
                    anyhow::Ok((0, Vec::new()))
                } else {
                    fs_err::tokio::write(filename, &new_content).await?;
                    anyhow::Ok((1, format!("Fixing {filename}\n").into_bytes()))
                }
            } else {
                // The file consists only of newlines. Normalize to a single newline.
                if content == b"\n" {
                    anyhow::Ok((0, Vec::new()))
                } else {
                    fs_err::tokio::write(filename, b"\n").await?;
                    anyhow::Ok((1, format!("Fixing {filename}\n").into_bytes()))
                }
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
