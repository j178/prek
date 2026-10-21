use std::path::{Path, PathBuf};

use anyhow::Result;
use clap::Parser;
use memchr::{memchr_iter, memrchr2};

use crate::hook::Hook;
use crate::hooks::{HookOutput, parse_hook_args, run_blocking_file_checks};

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Tab stop width in bytes; zero or negative values delete tabs.
    #[arg(
        long,
        value_name = "N",
        default_value_t = 4,
        allow_negative_numbers = true
    )]
    tab_width: i32,
    /// Report files that would change without modifying them.
    #[arg(long)]
    check: bool,
    #[arg(value_name = "FILENAMES")]
    filenames: Vec<PathBuf>,
}

pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;
    let width = usize::try_from(args.tab_width.max(0))?;
    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        move |file_path, display_path| fix_file(file_path, display_path, width, args.check),
    )
    .await
}

fn fix_file(
    file_path: &Path,
    display_path: &Path,
    width: usize,
    check: bool,
) -> Result<HookOutput> {
    let contents = fs_err::read(file_path)?;
    let mut tabs = memchr_iter(b'\t', &contents).peekable();
    if tabs.peek().is_none() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    if !check {
        let mut expanded = Vec::with_capacity(contents.len());
        let mut start = 0;
        for tab in tabs {
            let span = &contents[start..tab];
            expanded.extend_from_slice(span);
            if width > 0 {
                // Every previous tab ended on a tab stop. Only bytes since that
                // tab or the last CR/LF determine the padding, as in bytes.expandtabs().
                let column = match memrchr2(b'\r', b'\n', span) {
                    Some(newline) => span.len() - newline - 1,
                    None => span.len(),
                };
                let spaces = width - column % width;
                expanded.resize(expanded.len() + spaces, b' ');
            }
            start = tab + 1;
        }
        expanded.extend_from_slice(&contents[start..]);
        fs_err::write(file_path, expanded)?;
    }

    let action = if check { "Would fix" } else { "Fixing" };
    Ok(HookOutput::known(
        1,
        format!("{action} {}\n", display_path.display()).into_bytes(),
        !check,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expands_byte_columns_and_preserves_other_bytes() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("tabs");
        for (width, input, expected) in [
            (4, b"\tfoo".as_slice(), b"    foo".as_slice()),
            (4, b"foo\t", b"foo "),
            (4, b"foo \t  \t\t   bar", b"foo                bar"),
            (4, b"a\tb\r\n\tc\rd\t\n\te", b"a   b\r\n    c\rd   \n    e"),
            (
                4,
                b"\xc3\xa9\t\xe4\xb8\xad\t\xff\t",
                b"\xc3\xa9  \xe4\xb8\xad \xff   ",
            ),
            (4, b"a\x0b\tb\x0c\tc\0\t", b"a\x0b  b\x0c  c\0  "),
            (3, b"ab\t\tc\t", b"ab    c  "),
            (1, b"\ta\t\t", b" a  "),
        ] {
            fs_err::write(&path, input)?;
            let result = fix_file(&path, &path, width, false)?;
            assert_eq!(result.exit_status, 1);
            assert_eq!(
                fs_err::read(&path)?,
                expected,
                "width: {width}, input: {input:?}"
            );
        }
        Ok(())
    }
}
