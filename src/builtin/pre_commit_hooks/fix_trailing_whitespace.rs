use std::ops::Deref;
use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use bstr::ByteSlice;
use clap::Parser;
use futures::StreamExt;
use tempfile::NamedTempFile;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, BufWriter};

use crate::hook::Hook;
use crate::run::CONCURRENCY;

const MARKDOWN_LINE_BREAK: &[u8] = b"  ";
const BUFFER_SIZE_THRESHOLD: usize = 16 * 1024; // 16KB

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
    filenames: &[&String],
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
        .map(async |filename| fix_file(filename, &chars, force_markdown, &markdown_exts).await)
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
    filename: &str,
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

    let file = fs_err::tokio::File::open(filename).await?;
    let file_len = file.metadata().await?.len();
    let mut buf_writer = create_buffer(usize::try_from(file_len)?)?;
    let mut buf_reader = BufReader::new(file);

    let mut line = Vec::new();
    let mut modified = false;
    while buf_reader.read_until(b'\n', &mut line).await? != 0 {
        let line_ending = detect_line_ending(&line);
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

        buf_writer.write(trimmed).await?;
        if markdown_end {
            buf_writer.write(MARKDOWN_LINE_BREAK).await?;
            modified |= trimmed.len() + MARKDOWN_LINE_BREAK.len() + line_ending.len() != line.len();
        } else {
            modified |= trimmed.len() + line_ending.len() != line.len();
        }
        buf_writer.write(line_ending).await?;
        line.clear();
    }

    drop(buf_reader);
    if modified {
        buf_writer.flush_to_file(filename).await?;
        Ok((1, format!("Fixing {filename}\n").into_bytes()))
    } else {
        drop(buf_writer);
        Ok((0, Vec::new()))
    }
}

trait AsyncWriteBuffer {
    async fn write(&mut self, data: &[u8]) -> Result<()>;
    async fn flush_to_file(&mut self, filename: &str) -> Result<()>;
}

struct MemoryBuffer(Vec<u8>);

impl MemoryBuffer {
    pub fn new(mut file_len: usize) -> Self {
        if file_len > BUFFER_SIZE_THRESHOLD {
            file_len = BUFFER_SIZE_THRESHOLD;
        }
        Self(Vec::with_capacity(file_len))
    }
}

impl AsyncWriteBuffer for MemoryBuffer {
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.0.extend_from_slice(data);
        Ok(())
    }

    async fn flush_to_file(&mut self, filename: &str) -> Result<()> {
        fs_err::tokio::write(filename, &self.0).await?;
        Ok(())
    }
}

struct TempFileBuffer {
    buf_writer: BufWriter<tokio::fs::File>,
    named_temp_file: NamedTempFile,
}

impl TempFileBuffer {
    pub fn new() -> Result<Self> {
        let named_temp_file = NamedTempFile::new()?;
        let temp_file = tokio::fs::File::from_std(named_temp_file.reopen()?);
        let buf_writer = BufWriter::new(temp_file);

        Ok(Self {
            buf_writer,
            named_temp_file,
        })
    }
}

impl AsyncWriteBuffer for TempFileBuffer {
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        self.buf_writer.write_all(data).await?;
        Ok(())
    }

    async fn flush_to_file(&mut self, filename: &str) -> Result<()> {
        self.buf_writer.flush().await?;
        crate::fs::rename_or_copy(self.named_temp_file.path(), Path::new(filename)).await?;
        Ok(())
    }
}

enum Buffer {
    Memory(MemoryBuffer),
    Temp(TempFileBuffer),
}

impl AsyncWriteBuffer for Buffer {
    async fn write(&mut self, data: &[u8]) -> Result<()> {
        match self {
            Buffer::Memory(b) => b.write(data).await,
            Buffer::Temp(b) => b.write(data).await,
        }
    }

    async fn flush_to_file(&mut self, filename: &str) -> Result<()> {
        match self {
            Buffer::Memory(b) => b.flush_to_file(filename).await,
            Buffer::Temp(b) => b.flush_to_file(filename).await,
        }
    }
}

fn create_buffer(file_len: usize) -> Result<Buffer> {
    if file_len <= BUFFER_SIZE_THRESHOLD {
        Ok(Buffer::Memory(MemoryBuffer::new(file_len)))
    } else {
        Ok(Buffer::Temp(TempFileBuffer::new()?))
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

    async fn create_test_file(dir: &TempDir, name: &str, content: &[u8]) -> PathBuf {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await.unwrap();
        file_path
    }

    async fn run_fix_on_file(
        file_path: &Path,
        chars: &[char],
        force_markdown: bool,
        markdown_exts: &[String],
    ) -> (i32, Vec<u8>) {
        let filename = file_path.to_string_lossy().to_string();
        fix_file(&filename, chars, force_markdown, markdown_exts)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn test_trim_non_markdown_trims_spaces() {
        let dir = TempDir::new().unwrap();
        let file_path =
            create_test_file(&dir, "file.txt", b"keep this line\ntrim trailing    \n").await;

        let chars = vec![' ', '\t'];
        let md_exts = vec![".md".to_string()];

        let (code, msg) = run_fix_on_file(&file_path, &chars, false, &md_exts).await;

        // modified
        assert_eq!(code, 1);
        let msg_str = String::from_utf8_lossy(&msg);
        assert!(msg_str.contains("file.txt"));

        // file content updated: trailing spaces removed
        let content = fs_err::tokio::read_to_string(&file_path).await.unwrap();
        let expected = "keep this line\ntrim trailing\n";
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_markdown_preserve_two_spaces_and_reduce_extra() {
        let dir = TempDir::new().unwrap();
        let file_path = create_test_file(
            &dir,
            "doc.md",
            b"line_keep_two  \nline_reduce_three   \nother_line\n",
        )
        .await;

        let chars = vec![' ', '\t'];
        let md_exts = vec![".md".to_string()];

        let (code, _msg) = run_fix_on_file(&file_path, &chars, false, &md_exts).await;

        // second line changed 3 -> 2 spaces, so modified
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&file_path).await.unwrap();
        let expected = "line_keep_two  \nline_reduce_three  \nother_line\n";
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_force_markdown_obeys_markdown_rules() {
        let dir = TempDir::new().unwrap();
        // .txt normally not markdown, but we force markdown=true
        let file_path = create_test_file(
            &dir,
            "forced.txt",
            b"keep_two_spaces  \nthree_spaces_line   \n",
        )
        .await;

        let chars = vec![' ', '\t'];
        let md_exts: Vec<String> = vec![]; // irrelevant because force_markdown = true

        let (code, _msg) = run_fix_on_file(&file_path, &chars, true, &md_exts).await;

        // modified because one line had 3 spaces -> reduced to 2
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&file_path).await.unwrap();
        let expected = "keep_two_spaces  \nthree_spaces_line  \n";
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_no_changes_returns_zero_and_no_write() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "ok.txt", b"already_trimmed\nline_two\n").await;
        let chars = vec![' ', '\t'];
        let md_exts = vec![".md".to_string()];

        // file already trimmed -> no changes
        let (code, msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 0);
        assert!(msg.is_empty());

        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert_eq!(content, "already_trimmed\nline_two\n");
    }

    #[tokio::test]
    async fn test_empty_file_no_change() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "empty.txt", b"").await;
        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let (code, msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 0);
        assert!(msg.is_empty());
        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert_eq!(content, "");
    }

    #[tokio::test]
    async fn test_only_whitespace_lines_are_handled_not_markdown_end() {
        let dir = TempDir::new().unwrap();
        // lines are only whitespace; markdown_end_flag should NOT trigger
        let path = create_test_file(&dir, "ws.txt", b"   \n\t\n  \n").await;
        let chars = vec![' ', '\t'];
        let md_exts = vec![".md".to_string()];

        let (code, _msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        // trimming whitespace-only lines will change them to empty lines -> modified true
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        // Expect empty lines (newline preserved per implementation)
        assert_eq!(content, "\n\n\n");
    }

    #[tokio::test]
    async fn test_chars_empty_uses_trim_ascii_end() {
        let dir = TempDir::new().unwrap();
        // trailing ascii spaces should be removed by trim_ascii_end when chars is empty
        let path = create_test_file(&dir, "ascii.txt", b"foo   \nbar \t\n").await;
        let chars = vec![]; // will hit trim_ascii_end()
        let md_exts = vec![];

        let (code, _msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        let expected = "foo\nbar\n";
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_crlf_lines_handling() {
        let dir = TempDir::new().unwrap();
        // CRLF content (use \r\n). Ensure trimming still works.
        let path = create_test_file(&dir, "crlf.txt", b"one  \r\ntwo   \r\n").await;
        let chars = vec![' ', '\t'];
        let md_exts = vec![".txt".to_string()]; // treat as markdown for this test

        let (code, _msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 1);

        // read file and check logical lines presence (line endings may be normalized by lines())
        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert!(content.contains("one"));
        assert!(content.contains("two"));
    }

    #[tokio::test]
    async fn test_no_newline_at_eof() {
        let dir = TempDir::new().unwrap();
        // no trailing newline on last line
        let path = create_test_file(&dir, "no_nl.txt", b"lastline   ").await;
        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let (code, _msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        // Expect trailing spaces removed
        assert_eq!(content, "lastline");
    }

    #[tokio::test]
    async fn test_unicode_trim_char() {
        let dir = TempDir::new().unwrap();
        // use a unicode char '。' and ideographic space '　' to trim
        let path = create_test_file(&dir, "uni.txt", "hello。　\n".as_bytes()).await;
        let chars = vec!['。', '　'];
        let md_exts = vec![];

        let (code, _msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert_eq!(content, "hello\n");
    }

    #[tokio::test]
    async fn test_extension_case_insensitive_matching() {
        let dir = TempDir::new().unwrap();
        // capital extension .MD should match .md in markdown_exts
        let path = create_test_file(&dir, "Doc.MD", b"hi   \n").await;
        let chars = vec![' ', '\t'];
        let md_exts = vec![".md".to_string()];

        let (code, _msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        // markdown rules: trailing >2 -> reduce to two spaces
        assert!(content.contains("hi"));
    }

    #[tokio::test]
    async fn test_mixed_lines_modified_flag_true_if_any_changed() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "mix.txt", b"ok\nneedtrim   \nalso_ok\n").await;
        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let (code, _msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 1);

        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        let expected = "ok\nneedtrim\nalso_ok\n";
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_no_change_no_newline_at_eof() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "ok_no_nl.txt", b"foo\nbar").await;
        let chars = vec![' ', '\t'];
        let md_exts = vec![];

        let (code, msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 0);
        assert!(msg.is_empty());

        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert_eq!(content, "foo\nbar");
    }

    #[tokio::test]
    async fn test_markdown_wildcard_ext_and_eof_whitespace_removed() {
        let dir = TempDir::new().unwrap();
        let content = b"foo  \nbar \nbaz    \n\t\n\n  ";
        let path = create_test_file(&dir, "wild.md", content).await;
        let chars = vec![' ', '\t'];
        let md_exts = vec!["*".to_string()];

        let (code, _msg) = run_fix_on_file(&path, &chars, true, &md_exts).await;
        assert_eq!(code, 1);

        let expected = "foo  \nbar\nbaz  \n\n\n";
        let new_content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert_eq!(new_content, expected);
    }

    #[tokio::test]
    async fn test_markdown_with_custom_charset() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "custom_charset.md", b"\ta \t   \n").await;
        let chars = vec![' '];
        let md_exts = vec!["*".to_string()];

        let (code, _msg) = run_fix_on_file(&path, &chars, true, &md_exts).await;
        assert_eq!(code, 1);

        let expected = "\ta \t  \n";
        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_eol_trim() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "trim_eol.md", b"a\nb\r\r\r\n").await;
        let chars = vec!['x'];
        let md_exts = vec![];

        let (code, _msg) = run_fix_on_file(&path, &chars, true, &md_exts).await;
        assert_eq!(code, 0);

        let expected = "a\nb\r\r\r\n";
        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_markdown_trim() {
        let dir = TempDir::new().unwrap();
        let path = create_test_file(&dir, "trim_markdown.md", b"axxx  \n").await;
        let chars = vec!['x'];
        let md_exts = vec!["md".to_string()];

        let (code, _msg) = run_fix_on_file(&path, &chars, true, &md_exts).await;
        assert_eq!(code, 1);

        let expected = "a  \n";
        let content = fs_err::tokio::read_to_string(&path).await.unwrap();
        assert_eq!(content, expected);
    }

    #[tokio::test]
    async fn test_invalid_utf8_file_is_handled() {
        let dir = TempDir::new().unwrap();
        // This is valid ASCII followed by invalid UTF-8 (0xFF)
        let content = b"valid line\ninvalid utf8 here:\xff\n";
        let path = create_test_file(&dir, "invalid_utf8.txt", content).await;
        let chars = vec![' ', '\t'];
        let md_exts: Vec<String> = vec![];

        let (code, _msg) = run_fix_on_file(&path, &chars, false, &md_exts).await;
        assert_eq!(code, 0);

        let new_content = fs_err::tokio::read(&path).await.unwrap();
        // The invalid byte should still be present, but trailing whitespace should be trimmed
        assert!(new_content.starts_with(b"valid line\ninvalid utf8 here:\xff\n"));
    }
}
