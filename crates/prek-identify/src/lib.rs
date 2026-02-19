// Copyright (c) 2017 Chris Kuehl, Anthony Sottile
//
// Permission is hereby granted, free of charge, to any person obtaining a copy
// of this software and associated documentation files (the "Software"), to deal
// in the Software without restriction, including without limitation the rights
// to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
// copies of the Software, and to permit persons to whom the Software is
// furnished to do so, subject to the following conditions:
//
// The above copyright notice and this permission notice shall be included in
// all copies or substantial portions of the Software.
//
// THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
// IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
// FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
// AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
// LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
// OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN
// THE SOFTWARE.

use std::io::{BufRead, Read};
use std::iter::FromIterator;
use std::path::Path;

use smallvec::SmallVec;

pub use tags::ALL_TAGS;

mod tags;

#[derive(Clone, Default)]
pub struct TagSet(SmallVec<[&'static str; 8]>);

impl TagSet {
    fn new() -> Self {
        Self::default()
    }

    fn insert(&mut self, tag: &'static str) -> bool {
        if self.0.contains(&tag) {
            false
        } else {
            self.0.push(tag);
            true
        }
    }

    fn extend_from_iter<I>(&mut self, iter: I)
    where
        I: IntoIterator<Item = &'static str>,
    {
        for tag in iter {
            self.insert(tag);
        }
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.0.contains(&needle)
    }

    pub fn iter(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.0.iter().copied()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn with_added(mut self, extra: &[&'static str]) -> Self {
        self.extend_from_iter(extra.iter().copied());
        self
    }
}

impl Extend<&'static str> for TagSet {
    fn extend<I: IntoIterator<Item = &'static str>>(&mut self, iter: I) {
        self.extend_from_iter(iter);
    }
}

impl FromIterator<&'static str> for TagSet {
    fn from_iter<I: IntoIterator<Item = &'static str>>(iter: I) -> Self {
        let mut set = TagSet::new();
        set.extend(iter);
        set
    }
}

impl<const N: usize> From<[&'static str; N]> for TagSet {
    fn from(tags: [&'static str; N]) -> Self {
        tags.into_iter().collect()
    }
}

fn is_encoding_tag(tag: &str) -> bool {
    matches!(tag, "binary" | "text")
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error(transparent)]
    Shebang(#[from] ShebangError),
}

/// Identify tags for a file at the given path.
pub fn tags_from_path(path: &Path) -> Result<TagSet, Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.is_dir() {
        return Ok(TagSet::from(["directory"]));
    } else if metadata.is_symlink() {
        return Ok(TagSet::from(["symlink"]));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let file_type = metadata.file_type();
        if file_type.is_socket() {
            return Ok(TagSet::from(["socket"]));
        } else if file_type.is_fifo() {
            return Ok(TagSet::from(["fifo"]));
        } else if file_type.is_block_device() {
            return Ok(TagSet::from(["block-device"]));
        } else if file_type.is_char_device() {
            return Ok(TagSet::from(["character-device"]));
        }
    };

    let mut tags = TagSet::new();
    tags.insert("file");

    let executable;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        executable = metadata.permissions().mode() & 0o111 != 0;
    }
    #[cfg(not(unix))]
    {
        // `pre-commit/identify` uses `os.access(path, os.X_OK)` to check for executability on Windows.
        // This would actually return true for any file.
        // We keep this behavior for compatibility.
        executable = true;
    }

    if executable {
        tags.insert("executable");
    } else {
        tags.insert("non-executable");
    }

    let filename_tags = tags_from_filename(path);
    tags.extend(filename_tags.iter());
    if executable {
        if let Ok(shebang) = parse_shebang(path) {
            let interpreter_tags = tags_from_interpreter(shebang[0].as_str());
            tags.extend(interpreter_tags.iter());
        }
    }

    if !tags.iter().any(is_encoding_tag) {
        if is_text_file(path) {
            tags.insert("text");
        } else {
            tags.insert("binary");
        }
    }

    Ok(tags)
}

fn tags_from_filename(filename: &Path) -> TagSet {
    let ext = filename.extension().and_then(|ext| ext.to_str());
    let filename = filename
        .file_name()
        .and_then(|name| name.to_str())
        .expect("Invalid filename");

    let mut result = TagSet::new();

    if let Some(tags) = tags::NAMES.get(filename) {
        result.extend(tags.iter().copied());
    }
    if result.is_empty() {
        // # Allow e.g. "Dockerfile.xenial" to match "Dockerfile".
        if let Some(name) = filename.split('.').next() {
            if let Some(tags) = tags::NAMES.get(name) {
                result.extend(tags.iter().copied());
            }
        }
    }

    if let Some(ext) = ext {
        // Check if extension is already lowercase to avoid allocation
        if ext.chars().all(|c| c.is_ascii_lowercase()) {
            if let Some(tags) = tags::EXTENSIONS.get(ext) {
                result.extend(tags.iter().copied());
            }
        } else {
            let ext_lower = ext.to_ascii_lowercase();
            if let Some(tags) = tags::EXTENSIONS.get(ext_lower.as_str()) {
                result.extend(tags.iter().copied());
            }
        }
    }

    result
}

fn tags_from_interpreter(interpreter: &str) -> TagSet {
    let mut name = interpreter
        .rfind('/')
        .map(|pos| &interpreter[pos + 1..])
        .unwrap_or(interpreter);

    while !name.is_empty() {
        if let Some(tags) = tags::INTERPRETERS.get(name) {
            return tags.iter().copied().collect();
        }

        // python3.12.3 should match python3.12.3, python3.12, python3, python
        if let Some(pos) = name.rfind('.') {
            name = &name[..pos];
        } else {
            break;
        }
    }

    TagSet::new()
}

#[derive(thiserror::Error, Debug)]
pub enum ShebangError {
    #[error("No shebang found")]
    NoShebang,
    #[error("Shebang contains non-printable characters")]
    NonPrintableChars,
    #[error("Failed to parse shebang")]
    ParseFailed,
    #[error("No command found in shebang")]
    NoCommand,
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}

fn starts_with(slice: &[String], prefix: &[&str]) -> bool {
    slice.len() >= prefix.len() && slice.iter().zip(prefix.iter()).all(|(s, p)| s == p)
}

/// Parse nix-shell shebangs, which may span multiple lines.
/// See: <https://nixos.wiki/wiki/Nix-shell_shebang>
/// Example:
/// `#!nix-shell -i python3 -p python3` would return `["python3"]`
fn parse_nix_shebang<R: BufRead>(reader: &mut R, mut cmd: Vec<String>) -> Vec<String> {
    loop {
        let Ok(buf) = reader.fill_buf() else {
            break;
        };

        if buf.len() < 2 || &buf[..2] != b"#!" {
            break;
        }

        reader.consume(2);

        let mut next_line = String::new();
        match reader.read_line(&mut next_line) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) => {
                if err.kind() == std::io::ErrorKind::InvalidData {
                    return cmd;
                }
                break;
            }
        }

        let trimmed = next_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(line_tokens) = shlex::split(trimmed) {
            for idx in 0..line_tokens.len().saturating_sub(1) {
                if line_tokens[idx] == "-i" {
                    if let Some(interpreter) = line_tokens.get(idx + 1) {
                        cmd = vec![interpreter.clone()];
                    }
                }
            }
        }
    }

    cmd
}

pub fn parse_shebang(path: &Path) -> Result<Vec<String>, ShebangError> {
    let file = std::fs::File::open(path)?;
    let mut reader = std::io::BufReader::new(file);
    let mut line = String::new();
    reader.read_line(&mut line)?;
    if !line.starts_with("#!") {
        return Err(ShebangError::NoShebang);
    }

    // Require only printable ASCII
    if line
        .bytes()
        .any(|b| !(0x20..=0x7E).contains(&b) && !(0x09..=0x0D).contains(&b))
    {
        return Err(ShebangError::NonPrintableChars);
    }

    let mut tokens = shlex::split(line[2..].trim()).ok_or(ShebangError::ParseFailed)?;
    let mut cmd =
        if starts_with(&tokens, &["/usr/bin/env", "-S"]) || starts_with(&tokens, &["env", "-S"]) {
            tokens.drain(0..2);
            tokens
        } else if starts_with(&tokens, &["/usr/bin/env"]) || starts_with(&tokens, &["env"]) {
            tokens.drain(0..1);
            tokens
        } else {
            tokens
        };
    if cmd.is_empty() {
        return Err(ShebangError::NoCommand);
    }
    if cmd[0] == "nix-shell" {
        cmd = parse_nix_shebang(&mut reader, cmd);
    }
    if cmd.is_empty() {
        return Err(ShebangError::NoCommand);
    }

    Ok(cmd)
}

// Lookup table for text character detection.
static IS_TEXT_CHAR: [u32; 8] = {
    let mut table = [0u32; 8];
    let mut i = 0;
    while i < 256 {
        // Printable ASCII (0x20..0x7F)
        // High bit set (>= 0x80)
        // Control characters: 7, 8, 9, 10, 11, 12, 13, 27
        let is_text =
            (i >= 0x20 && i < 0x7F) || i >= 0x80 || matches!(i, 7 | 8 | 9 | 10 | 11 | 12 | 13 | 27);
        if is_text {
            table[i / 32] |= 1 << (i % 32);
        }
        i += 1;
    }
    table
};

fn is_text_char(b: u8) -> bool {
    let idx = b as usize;
    (IS_TEXT_CHAR[idx / 32] & (1 << (idx % 32))) != 0
}

/// Return whether the first KB of contents seems to be binary.
///
/// This is roughly based on libmagic's binary/text detection:
/// <https://github.com/file/file/blob/df74b09b9027676088c797528edcaae5a9ce9ad0/src/encoding.c#L203-L228>
fn is_text_file(path: &Path) -> bool {
    let mut buffer = [0; 1024];
    let Ok(mut file) = fs_err::File::open(path) else {
        return false;
    };

    let Ok(bytes_read) = file.read(&mut buffer) else {
        return false;
    };
    if bytes_read == 0 {
        return true;
    }

    buffer[..bytes_read].iter().all(|&b| is_text_char(b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::Path;

    fn assert_tagset(actual: &TagSet, expected: &[&'static str]) {
        let mut actual_vec: Vec<_> = actual.iter().collect();
        actual_vec.sort_unstable();
        let mut expected_vec = expected.to_vec();
        expected_vec.sort_unstable();
        assert_eq!(actual_vec, expected_vec);
    }

    #[test]
    #[cfg(unix)]
    fn tags_from_path() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("source.txt");
        let dest = dir.path().join("link.txt");
        fs_err::File::create(&src)?;
        std::os::unix::fs::symlink(&src, &dest)?;

        let tags = super::tags_from_path(dir.path())?;
        assert_tagset(&tags, &["directory"]);
        let tags = super::tags_from_path(&src)?;
        assert_tagset(&tags, &["plain-text", "non-executable", "file", "text"]);
        let tags = super::tags_from_path(&dest)?;
        assert_tagset(&tags, &["symlink"]);

        Ok(())
    }

    #[test]
    #[cfg(windows)]
    fn tags_from_path() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let src = dir.path().join("source.txt");
        fs_err::File::create(&src)?;

        let tags = super::tags_from_path(dir.path())?;
        assert_tagset(&tags, &["directory"]);
        let tags = super::tags_from_path(&src)?;
        assert_tagset(&tags, &["plain-text", "executable", "file", "text"]);

        Ok(())
    }

    #[test]
    fn tags_from_filename() {
        let tags = super::tags_from_filename(Path::new("test.py"));
        assert_tagset(&tags, &["python", "text"]);

        let tags = super::tags_from_filename(Path::new("bitbake.bbappend"));
        assert_tagset(&tags, &["bitbake", "text"]);

        let tags = super::tags_from_filename(Path::new("project.fsproj"));
        assert_tagset(&tags, &["fsproj", "msbuild", "text", "xml"]);

        let tags = super::tags_from_filename(Path::new("data.json"));
        assert_tagset(&tags, &["json", "text"]);

        let tags = super::tags_from_filename(Path::new("build.props"));
        assert_tagset(&tags, &["msbuild", "text", "xml"]);

        let tags = super::tags_from_filename(Path::new("profile.psd1"));
        assert_tagset(&tags, &["powershell", "text"]);

        let tags = super::tags_from_filename(Path::new("style.xslt"));
        assert_tagset(&tags, &["text", "xml", "xsl"]);

        let tags = super::tags_from_filename(Path::new("Pipfile"));
        assert_tagset(&tags, &["toml", "text"]);

        let tags = super::tags_from_filename(Path::new("Pipfile.lock"));
        assert_tagset(&tags, &["json", "text"]);

        let tags = super::tags_from_filename(Path::new("file.pdf"));
        assert_tagset(&tags, &["pdf", "binary"]);

        let tags = super::tags_from_filename(Path::new("FILE.PDF"));
        assert_tagset(&tags, &["pdf", "binary"]);

        let tags = super::tags_from_filename(Path::new(".envrc"));
        assert_tagset(&tags, &["bash", "shell", "text"]);

        let tags = super::tags_from_filename(Path::new("meson.options"));
        assert_tagset(&tags, &["meson", "meson-options", "text"]);

        let tags = super::tags_from_filename(Path::new("Tiltfile"));
        assert_tagset(&tags, &["text", "tiltfile"]);

        let tags = super::tags_from_filename(Path::new("Tiltfile.dev"));
        assert_tagset(&tags, &["text", "tiltfile"]);
    }

    #[test]
    fn tags_from_interpreter() {
        let tags = super::tags_from_interpreter("/usr/bin/python3");
        assert_tagset(&tags, &["python", "python3"]);

        let tags = super::tags_from_interpreter("/usr/bin/python3.12");
        assert_tagset(&tags, &["python", "python3"]);

        let tags = super::tags_from_interpreter("/usr/bin/python3.12.3");
        assert_tagset(&tags, &["python", "python3"]);

        let tags = super::tags_from_interpreter("python");
        assert_tagset(&tags, &["python"]);

        let tags = super::tags_from_interpreter("sh");
        assert_tagset(&tags, &["shell", "sh"]);

        let tags = super::tags_from_interpreter("invalid");
        assert!(tags.is_empty());
    }

    #[test]
    fn parse_shebang_nix_shell_interpreter() -> anyhow::Result<()> {
        let mut file = tempfile::NamedTempFile::new()?;
        writeln!(
            file,
            indoc::indoc! {r#"
            #!/usr/bin/env nix-shell
            #! nix-shell --pure -i bash -p "python3.withPackages (p: [ p.numpy p.sympy ])"
            #! nix-shell -I nixpkgs=https://example.com
            echo hi
            "#}
        )?;
        file.flush()?;

        let cmd = super::parse_shebang(file.path())?;
        assert_eq!(cmd, vec!["bash"]);

        Ok(())
    }

    #[test]
    fn parse_shebang_nix_shell_without_interpreter() -> anyhow::Result<()> {
        let mut file = tempfile::NamedTempFile::new()?;
        writeln!(
            file,
            indoc::indoc! {r"
            #!/usr/bin/env nix-shell -p python3
            #! nix-shell --pure -I nixpkgs=https://example.com
            echo hi
            "}
        )?;
        file.flush()?;

        let cmd = super::parse_shebang(file.path())?;
        assert_eq!(cmd, vec!["nix-shell", "-p", "python3"]);

        Ok(())
    }
}
