use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use bstr::ByteSlice;
use clap::Parser;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

const MARKDOWN_LINE_BREAK: &[u8] = b"  ";

#[derive(Clone)]
struct Chars(Vec<char>);

impl FromStr for Chars {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Chars(s.chars().collect()))
    }
}

impl Deref for Chars {
    type Target = Vec<char>;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long)]
    markdown_linebreak_ext: Vec<String>,
    // `clap` cannot parse `--chars= \t` into vec<char> correctly.
    // so, we use Chars to achieve it.
    #[arg(long)]
    chars: Option<Chars>,
}

impl Args {
    fn markdown_exts(&self) -> Result<Vec<String>> {
        let markdown_exts = self
            .markdown_linebreak_ext
            .iter()
            .flat_map(|ext| ext.split(','))
            .map(|ext| format!(".{}", ext.trim_start_matches('.')).to_ascii_lowercase())
            .collect::<Vec<_>>();

        // Validate extensions don't contain path separators
        for ext in &markdown_exts {
            if ext[1..]
                .chars()
                .any(|c| matches!(c, '.' | '/' | '\\' | ':'))
            {
                return Err(anyhow::anyhow!(
                    "bad `--markdown-linebreak-ext` argument '{ext}' (has . / \\ :)"
                ));
            }
        }
        Ok(markdown_exts)
    }

    fn force_markdown(&self) -> bool {
        self.markdown_linebreak_ext.iter().any(|ext| ext == "*")
    }
}

pub(crate) async fn fix_trailing_whitespace(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;

    let force_markdown = args.force_markdown();
    let markdown_exts = args.markdown_exts()?;
    let chars = if let Some(chars) = args.chars {
        chars.deref().to_owned()
    } else {
        Vec::new()
    };

    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| {
            fix_file(
                hook.project().relative_path(),
                filename,
                &chars,
                force_markdown,
                &markdown_exts,
            )
            .await
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

async fn fix_file(
    file_base: &Path,
    filename: &Path,
    chars: &[char],
    force_markdown: bool,
    markdown_exts: &[String],
) -> Result<(i32, Vec<u8>)> {
    let is_markdown = force_markdown || {
        Path::new(filename)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| format!(".{}", e.to_ascii_lowercase()))
            .is_some_and(|e| markdown_exts.contains(&e))
    };

    let file_path = file_base.join(filename);
    let content = fs_err::tokio::read(&file_path).await?;

    let mut output = Vec::with_capacity(content.len());
    let mut modified = false;
    for line in content.split_inclusive(|&b| b == b'\n') {
        let line_ending = detect_line_ending(line);
        let mut trimmed = &line[..line.len() - line_ending.len()];

        let markdown_end = needs_markdown_break(is_markdown, trimmed);
        if markdown_end {
            trimmed = &trimmed[..trimmed.len() - MARKDOWN_LINE_BREAK.len()];
        }

        if chars.is_empty() {
            trimmed = trimmed.trim_ascii_end();
        } else {
            trimmed = trimmed.trim_end_with(|c| chars.contains(&c));
        }

        output.extend_from_slice(trimmed);
        if markdown_end {
            output.extend_from_slice(MARKDOWN_LINE_BREAK);
            modified |= trimmed.len() + MARKDOWN_LINE_BREAK.len() + line_ending.len() != line.len();
        } else {
            modified |= trimmed.len() + line_ending.len() != line.len();
        }
        output.extend_from_slice(line_ending);
    }

    if modified {
        fs_err::tokio::write(&file_path, &output).await?;
        Ok((1, format!("Fixing {}\n", filename.display()).into_bytes()))
    } else {
        Ok((0, Vec::new()))
    }
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
    is_markdown
        && !trimmed.chars().all(|b| b.is_ascii_whitespace())
        && trimmed.ends_with(MARKDOWN_LINE_BREAK)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use tempfile::TempDir;

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
        let md_exts = vec![".md".to_string()];

        let (code, msg) = fix_file(Path::new(""), &file_path, &chars, false, &md_exts).await?;

        // modified
        assert_eq!(code, 1);
        let msg_str = String::from_utf8_lossy(&msg);
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
        let md_exts = vec![".md".to_string()];

        let (code, _msg) = fix_file(Path::new(""), &file_path, &chars, false, &md_exts).await?;

        // second line changed 3 -> 2 spaces, so modified
        assert_eq!(code, 1);

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
        let md_exts: Vec<String> = vec![]; // irrelevant because force_markdown = true

        let (code, _msg) = fix_file(Path::new(""), &file_path, &chars, true, &md_exts).await?;

        // modified because one line had 3 spaces -> reduced to 2
        assert_eq!(code, 1);

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
        let md_exts = vec![".md".to_string()];

        // file already trimmed -> no changes
        let (code, msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 0);
        assert!(msg.is_empty());

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

        let (code, msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 0);
        assert!(msg.is_empty());
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
        let md_exts = vec![".md".to_string()];

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        // trimming whitespace-only lines will change them to empty lines -> modified true
        assert_eq!(code, 1);

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

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 1);

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
        let md_exts = vec![".txt".to_string()]; // treat as markdown for this test

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 1);

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

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&path).await?;
        // Expect trailing spaces removed
        assert_eq!(content, "lastline");

        Ok(())
    }

    #[tokio::test]
    async fn test_unicode_trim_char() -> Result<()> {
        let dir = TempDir::new()?;
        // use a unicode char '。' and ideographic space '　' to trim
        let path = create_test_file(&dir, "uni.txt", "hello。　\n".as_bytes()).await?;
        let chars = vec!['。', '　'];
        let md_exts = vec![];

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, "hello\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_extension_case_insensitive_matching() -> Result<()> {
        let dir = TempDir::new()?;
        // capital extension .MD should match .md in markdown_exts
        let path = create_test_file(&dir, "Doc.MD", b"hi   \n").await?;
        let chars = vec![' ', '\t'];
        let md_exts = vec![".md".to_string()];

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 1);

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

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 1);

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

        let (code, msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 0);
        assert!(msg.is_empty());

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
        let md_exts = vec!["*".to_string()];

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, true, &md_exts).await?;
        assert_eq!(code, 1);

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
        let md_exts = vec!["*".to_string()];

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, true, &md_exts).await?;
        assert_eq!(code, 1);

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

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, true, &md_exts).await?;
        assert_eq!(code, 0);

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
        let md_exts = vec!["md".to_string()];

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, true, &md_exts).await?;
        assert_eq!(code, 1);

        let expected = "a  \n";
        let content = fs_err::tokio::read_to_string(&path).await?;
        assert_eq!(content, expected);

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_utf8_file_is_handled() -> Result<()> {
        let dir = TempDir::new()?;
        // This is valid ASCII followed by invalid UTF-8 (0xFF)
        let content = b"valid line\ninvalid utf8 here:\xff\n";
        let path = create_test_file(&dir, "invalid_utf8.txt", content).await?;
        let chars = vec![' ', '\t'];
        let md_exts: Vec<String> = vec![];

        let (code, _msg) = fix_file(Path::new(""), &path, &chars, false, &md_exts).await?;
        assert_eq!(code, 0);

        let new_content = fs_err::tokio::read(&path).await?;
        // The invalid byte should still be present, but trailing whitespace should be trimmed
        assert!(new_content.starts_with(b"valid line\ninvalid utf8 here:\xff\n"));

        Ok(())
    }
}
