use std::path::{Path, PathBuf};
use std::str::FromStr;

use anyhow::Result;
use bstr::ByteSlice;
use clap::Parser;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{parse_hook_args, run_blocking_file_checks};

const MARKDOWN_LINE_BREAK: &[u8] = b"  ";

#[derive(Clone)]
struct Chars(Vec<char>);

impl FromStr for Chars {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Chars(s.chars().collect()))
    }
}

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Report files that would change without modifying them.
    #[arg(long)]
    check: bool,
    /// Preserve Markdown hard line breaks for EXT (repeatable).
    #[arg(long, value_name = "EXT")]
    markdown_linebreak_ext: Vec<String>,
    // `clap` cannot parse `--chars= \t` into vec<char> correctly.
    // so, we use Chars to achieve it.
    /// Trim only these characters.
    #[arg(long)]
    chars: Option<Chars>,
    #[arg(value_name = "FILENAMES")]
    filenames: Vec<PathBuf>,
}

impl Args {
    fn markdown_exts(&self) -> Result<Vec<String>> {
        let markdown_exts = self
            .markdown_linebreak_ext
            .iter()
            .flat_map(|ext| ext.split(','))
            .map(|ext| ext.trim_start_matches('.').to_owned())
            .collect::<Vec<_>>();

        // Validate extensions don't contain path separators
        for ext in &markdown_exts {
            if ext.chars().any(|c| matches!(c, '.' | '/' | '\\' | ':')) {
                anyhow::bail!("bad `--markdown-linebreak-ext` argument '{ext}' (has . / \\ :)");
            }
        }
        Ok(markdown_exts)
    }

    fn force_markdown(&self) -> bool {
        self.markdown_linebreak_ext.iter().any(|ext| ext == "*")
    }
}

/// Runs the `trailing-whitespace` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;

    let force_markdown = args.force_markdown();
    let markdown_exts = args.markdown_exts()?;
    let chars = args.chars.map_or_else(Vec::new, |chars| chars.0);

    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        move |file_path, display_path| {
            fix_file(
                file_path,
                display_path,
                &chars,
                force_markdown,
                &markdown_exts,
                args.check,
            )
        },
    )
    .await
}

fn fix_file(
    file_path: &Path,
    display_path: &Path,
    chars: &[char],
    force_markdown: bool,
    markdown_exts: &[String],
    check: bool,
) -> Result<HookOutput> {
    let is_markdown = force_markdown || is_markdown_file(display_path, markdown_exts);

    let mut content = fs_err::read(file_path)?;
    let mut line_start = if chars.is_empty() && !is_markdown {
        let Some(start) = first_line_with_trailing_whitespace(&content) else {
            return Ok(HookOutput::unchanged(0, Vec::new()));
        };
        start
    } else {
        0
    };
    let mut copied = 0;
    let mut written = 0;
    while line_start < content.len() {
        let line_len = memchr::memchr(b'\n', &content[line_start..])
            .map_or(content.len() - line_start, |index| index + 1);
        let line_end = line_start + line_len;
        let line = &content[line_start..line_end];
        let line_ending = detect_line_ending(line);
        let mut trimmed = &line[..line.len() - line_ending.len()];

        let markdown_end = needs_markdown_break(is_markdown, trimmed);
        if markdown_end {
            trimmed = &trimmed[..trimmed.len() - MARKDOWN_LINE_BREAK.len()];
        }
        let suffix_start = line_start + trimmed.len();
        if chars.is_empty() {
            trimmed = trimmed.trim_ascii_end();
        } else {
            trimmed = trimmed.trim_end_with(|c| chars.contains(&c));
        }
        let trimmed_end = line_start + trimmed.len();

        if trimmed_end != suffix_start {
            if check {
                return Ok(HookOutput::unchanged(
                    1,
                    format!("Would fix {}\n", display_path.display()).into_bytes(),
                ));
            }
            // Compact unchanged runs in place. The destination never reaches unread bytes.
            content.copy_within(copied..trimmed_end, written);
            written += trimmed_end - copied;
            copied = suffix_start;
        }
        line_start = line_end;
    }

    if copied == 0 {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }
    content.copy_within(copied.., written);
    content.truncate(written + content.len() - copied);
    fs_err::write(file_path, &content)?;
    Ok(HookOutput::known(
        1,
        format!("Fixing {}\n", display_path.display()).into_bytes(),
        true,
    ))
}

fn first_line_with_trailing_whitespace(content: &[u8]) -> Option<usize> {
    let mut start = 0;
    for index in memchr::memchr_iter(b'\n', content).chain(std::iter::once(content.len())) {
        let prefix = &content[..index];
        let body = prefix.strip_suffix(b"\r").unwrap_or(prefix);
        // LF separates lines; one CR belongs to the line ending, even at EOF.
        if matches!(body.last(), Some(b' ' | b'\t' | b'\x0c' | b'\r')) {
            return Some(start);
        }
        start = index + 1;
    }
    None
}

fn is_markdown_file(filename: &Path, markdown_exts: &[String]) -> bool {
    filename
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|extension| {
            markdown_exts
                .iter()
                .any(|markdown_ext| markdown_ext.eq_ignore_ascii_case(extension))
        })
}

fn detect_line_ending(line: &[u8]) -> &[u8] {
    if line.ends_with(b"\r\n") {
        b"\r\n"
    } else if line.ends_with(b"\n") {
        b"\n"
    } else if line.ends_with(b"\r") {
        b"\r"
    } else {
        b""
    }
}

fn needs_markdown_break(is_markdown: bool, trimmed: &[u8]) -> bool {
    is_markdown && trimmed.ends_with(MARKDOWN_LINE_BREAK) && !trimmed.trim_ascii_start().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use tempfile::TempDir;

    #[test]
    fn skip_clean_prefix_before_trailing_whitespace() -> Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("sparse.txt");
        let prefix = b"unchanged\r\n\n\r\n\x0b\n".repeat(8192);
        for ending in [b"\n".as_slice(), b"\r\n", b"\r", b""] {
            let original = [prefix.as_slice(), b"\xfflast \t\x0c", ending].concat();
            fs_err::write(&path, &original)?;

            assert_eq!(
                fix_file(&path, &path, &[], false, &[], true)?.exit_status,
                1
            );
            assert_eq!(fs_err::read(&path)?, original);
            assert_eq!(
                fix_file(&path, &path, &[], false, &[], false)?.exit_status,
                1
            );
            assert_eq!(
                fs_err::read(&path)?,
                [prefix.as_slice(), b"\xfflast", ending].concat()
            );
            assert_eq!(
                fix_file(&path, &path, &[], false, &[], false)?.exit_status,
                0
            );
        }
        Ok(())
    }

    #[test]
    fn compact_sparse_whitespace_preserves_intervening_bytes() -> Result<()> {
        let dir = TempDir::new()?;
        let path = dir.path().join("sparse.md");
        let middle = b"unchanged\r\n".repeat(8192);
        let original = [b"first   \r\n", middle.as_slice(), b"\xfflast\t  \n   "].concat();
        fs_err::write(&path, &original)?;

        let result = fix_file(&path, &path, &[], true, &[], true)?;
        assert_eq!(result.exit_status, 1);
        assert_eq!(fs_err::read(&path)?, original);

        let result = fix_file(&path, &path, &[], true, &[], false)?;
        assert_eq!(result.exit_status, 1);
        assert_eq!(
            fs_err::read(&path)?,
            [b"first  \r\n", middle.as_slice(), b"\xfflast  \n"].concat()
        );
        assert_eq!(
            fix_file(&path, &path, &[], true, &[], false)?.exit_status,
            0
        );
        Ok(())
    }

    async fn create_test_file(dir: &TempDir, name: &str, content: &[u8]) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_trim_non_markdown_trims_spaces() -> Result<()> {
        let dir = TempDir::new()?;
        let file_path =
            create_test_file(&dir, "file.txt", b"keep this line\ntrim trailing    \n").await?;

        let chars = vec![' ', '\t'];
        let md_exts = vec!["md".to_owned()];

        let result = fix_file(&file_path, &file_path, &chars, false, &md_exts, false)?;

        // modified
        assert_eq!(result.exit_status, 1);
        let msg_str = String::from_utf8_lossy(&result.output);
        assert!(msg_str.contains("file.txt"));

        // file content updated: trailing spaces removed
        let content = fs_err::tokio::read_to_string(&file_path).await?;
        let expected = "keep this line\ntrim trailing\n";
        assert_eq!(content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_markdown_preserve_two_spaces_and_reduce_extra() -> Result<()> {
        let dir = TempDir::new()?;
        let file_path = create_test_file(
            &dir,
            "doc.md",
            b"line_keep_two  \nline_reduce_three   \nother_line\n",
        )
        .await?;

        let chars = vec![' ', '\t'];
        let md_exts = vec!["md".to_owned()];

        let result = fix_file(&file_path, &file_path, &chars, false, &md_exts, false)?;

        // second line changed 3 -> 2 spaces, so modified
        assert_eq!(result.exit_status, 1);

        let content = fs_err::tokio::read_to_string(&file_path).await?;
        let expected = "line_keep_two  \nline_reduce_three  \nother_line\n";
        assert_eq!(content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_force_markdown_obeys_markdown_rules() -> Result<()> {
        let dir = TempDir::new()?;
        // .txt normally not markdown, but we force markdown=true
        let file_path = create_test_file(
            &dir,
            "forced.txt",
            b"keep_two_spaces  \nthree_spaces_line   \n",
        )
        .await?;

        let chars = vec![' ', '\t'];
        let md_exts = vec![]; // irrelevant because force_markdown = true

        let result = fix_file(&file_path, &file_path, &chars, true, &md_exts, false)?;

        // modified because one line had 3 spaces -> reduced to 2
        assert_eq!(result.exit_status, 1);

        let content = fs_err::tokio::read_to_string(&file_path).await?;
        let expected = "keep_two_spaces  \nthree_spaces_line  \n";
        assert_eq!(content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_no_changes_returns_zero_and_no_write() -> Result<()> {
        let dir = TempDir::new()?;
        let path = create_test_file(&dir, "ok.txt", b"already_trimmed\nline_two\n").await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec!["md".to_owned()];

        // file already trimmed -> no changes
        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, "already_trimmed\nline_two\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file_no_change() -> Result<()> {
        let dir = TempDir::new()?;
        let path = create_test_file(&dir, "empty.txt", b"").await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());
        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, "");

        Ok(())
    }

    #[tokio::test]
    async fn test_only_whitespace_lines_are_handled_not_markdown_end() -> Result<()> {
        let dir = TempDir::new()?;
        // lines are only whitespace; markdown_end_flag should NOT trigger
        let path = create_test_file(&dir, "ws.txt", b"   \n\t\n  \n").await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec!["md".to_owned()];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        // trimming whitespace-only lines will change them to empty lines -> modified true
        assert_eq!(result.exit_status, 1);

        let content = fs_err::tokio::read_to_string(&path).await?;
        // Expect empty lines (newline preserved per implementation)
        assert_eq!(content, "\n\n\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_chars_empty_uses_trim_ascii_end() -> Result<()> {
        let dir = TempDir::new()?;
        // trailing ascii spaces should be removed by trim_ascii_end when chars is empty
        let path = create_test_file(&dir, "ascii.txt", b"foo   \nbar \t\n").await?;
        let chars = vec![]; // will hit trim_ascii_end()
        let md_exts = vec![];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        let content = fs_err::tokio::read_to_string(&path).await?;
        let expected = "foo\nbar\n";
        assert_eq!(content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_crlf_lines_handling() -> Result<()> {
        let dir = TempDir::new()?;
        // CRLF content (use \r\n). Ensure trimming still works.
        let path = create_test_file(&dir, "crlf.txt", b"one  \r\ntwo   \r\n").await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec!["txt".to_owned()]; // treat as markdown for this test

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        // read file and check logical lines presence (line endings may be normalized by lines())
        let content = fs_err::tokio::read_to_string(&path).await?;
        assert!(content.contains("one"));
        assert!(content.contains("two"));

        Ok(())
    }

    #[tokio::test]
    async fn test_no_newline_at_eof() -> Result<()> {
        let dir = TempDir::new()?;
        // no trailing newline on last line
        let path = create_test_file(&dir, "no_nl.txt", b"lastline   ").await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        let content = fs_err::tokio::read_to_string(&path).await?;
        // Expect trailing spaces removed
        assert_eq!(content, "lastline");

        Ok(())
    }

    #[tokio::test]
    async fn test_unicode_trim_char() -> Result<()> {
        let dir = TempDir::new()?;
        // use a Unicode char '。' and ideographic space '　' to trim
        let path = create_test_file(&dir, "uni.txt", "hello。　\n".as_bytes()).await?;
        let chars = vec!['。', '　'];
        let md_exts = vec![];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, "hello\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_extension_case_insensitive_matching() -> Result<()> {
        let dir = TempDir::new()?;
        // Capital extension .MD should match dotless `md`.
        let path = create_test_file(&dir, "Doc.MD", b"hi   \n").await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec!["md".to_owned()];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        let content = fs_err::tokio::read_to_string(&path).await?;
        // markdown rules: trailing >2 -> reduce to two spaces
        assert!(content.contains("hi"));

        Ok(())
    }

    #[tokio::test]
    async fn test_mixed_lines_modified_flag_true_if_any_changed() -> Result<()> {
        let dir = TempDir::new()?;
        let path = create_test_file(&dir, "mix.txt", b"ok\nneedtrim   \nalso_ok\n").await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        let content = fs_err::tokio::read_to_string(&path).await?;
        let expected = "ok\nneedtrim\nalso_ok\n";
        assert_eq!(content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_no_change_no_newline_at_eof() -> Result<()> {
        let dir = TempDir::new()?;
        let path = create_test_file(&dir, "ok_no_nl.txt", b"foo\nbar").await?;

        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, "foo\nbar");

        Ok(())
    }

    #[tokio::test]
    async fn test_markdown_wildcard_ext_and_eof_whitespace_removed() -> Result<()> {
        let dir = TempDir::new()?;
        let content = b"foo  \nbar \nbaz    \n\t\n\n  ";
        let path = create_test_file(&dir, "wild.md", content).await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec!["*".to_owned()];

        let result = fix_file(&path, &path, &chars, true, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        let expected = "foo  \nbar\nbaz  \n\n\n";
        let new_content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(new_content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_markdown_with_custom_charset() -> Result<()> {
        let dir = TempDir::new()?;
        let path = create_test_file(&dir, "custom_charset.md", b"\ta \t   \n").await?;
        let chars = vec![' '];
        let md_exts = vec!["*".to_owned()];

        let result = fix_file(&path, &path, &chars, true, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        let expected = "\ta \t  \n";
        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_eol_trim() -> Result<()> {
        let dir = TempDir::new()?;
        let path = create_test_file(&dir, "trim_eol.md", b"a\nb\r\r\r\n").await?;
        let chars = vec!['x'];
        let md_exts = vec![];

        let result = fix_file(&path, &path, &chars, true, &md_exts, false)?;
        assert_eq!(result.exit_status, 0);

        let expected = "a\nb\r\r\r\n";
        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_markdown_trim() -> Result<()> {
        let dir = TempDir::new()?;
        let path = create_test_file(&dir, "trim_markdown.md", b"axxx  \n").await?;
        let chars = vec!['x'];
        let md_exts = vec!["md".to_owned()];

        let result = fix_file(&path, &path, &chars, true, &md_exts, false)?;
        assert_eq!(result.exit_status, 1);

        let expected = "a  \n";
        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, expected);

        Ok(())
    }

    #[test]
    fn test_markdown_extension_matching_uses_dotless_extensions() {
        let md_exts = vec!["MD".to_owned()];
        assert!(is_markdown_file(Path::new("README.md"), &md_exts));

        let md_exts = vec![".md".to_owned()];
        assert!(!is_markdown_file(Path::new("README.md"), &md_exts));
    }

    #[tokio::test]
    async fn test_invalid_utf8_file_is_handled() -> Result<()> {
        let dir = TempDir::new()?;
        // This is valid ASCII followed by invalid UTF-8 (0xFF)
        let content = b"valid line\ninvalid utf8 here:\xff\n";
        let path = create_test_file(&dir, "invalid_utf8.txt", content).await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let result = fix_file(&path, &path, &chars, false, &md_exts, false)?;
        assert_eq!(result.exit_status, 0);

        let new_content = fs_err::tokio::read(&path).await?;
        // The invalid byte should still be present, but trailing whitespace should be trimmed
        assert!(new_content.starts_with(b"valid line\ninvalid utf8 here:\xff\n"));

        Ok(())
    }
}
