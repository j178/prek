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

use std::borrow::Cow;
use std::io::{BufRead, Read};
use std::ops::BitOrAssign;
use std::path::Path;

#[cfg(feature = "serde")]
use serde::de::{Error as DeError, SeqAccess, Visitor};

pub mod tags;

const TAG_WORDS: usize = tags::ALL_TAGS.len().div_ceil(64);

/// A compact set of file tags represented as a fixed-size bitset.
///
/// Each bit corresponds to an index in [`tags::ALL_TAGS`].
/// This keeps membership / set operations fast and allocation-free.
#[derive(Clone, Copy, Default)]
pub struct TagSet {
    bits: [u64; TAG_WORDS],
}

impl std::fmt::Debug for TagSet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.iter()).finish()
    }
}

fn tag_id(tag: &str) -> Option<usize> {
    tags::ALL_TAGS.binary_search(&tag).ok()
}

pub struct TagSetIter<'a> {
    bits: &'a [u64; TAG_WORDS],
    word_idx: usize,
    cur_word: u64,
}

impl Iterator for TagSetIter<'_> {
    type Item = &'static str;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if self.cur_word != 0 {
                // Find index of the least-significant set bit in the current 64-bit word.
                let tz = self.cur_word.trailing_zeros() as usize;
                // Clear that least-significant set bit so the next call advances to the next tag.
                self.cur_word &= self.cur_word - 1;

                // `word_idx` is already incremented when `cur_word` was loaded,
                // so we use `word_idx - 1` here to compute the global tag index.
                let idx = (self.word_idx.saturating_sub(1) * 64) + tz;
                return tags::ALL_TAGS.get(idx).copied();
            }

            if self.word_idx >= TAG_WORDS {
                return None;
            }

            self.cur_word = self.bits[self.word_idx];
            self.word_idx += 1;
        }
    }
}

impl TagSet {
    /// Constructs a [`TagSet`] from tag ids.
    ///
    /// `tag_ids` must reference valid indexes in [`tags::ALL_TAGS_BY_ID`].
    /// Duplicate ids are allowed and are automatically coalesced.
    pub const fn new(tag_ids: &[u16]) -> Self {
        let mut bits = [0u64; TAG_WORDS];
        let mut idx = 0;
        while idx < tag_ids.len() {
            let tag_id = tag_ids[idx] as usize;
            assert!(tag_id < tags::ALL_TAGS.len(), "tag id out of range");
            bits[tag_id / 64] |= 1u64 << (tag_id % 64);
            idx += 1;
        }

        Self { bits }
    }

    fn empty() -> Self {
        Self::default()
    }

    /// Constructs a [`TagSet`] from tag strings.
    ///
    /// Unknown tags are ignored in release builds and debug-asserted in debug builds.
    pub fn from_tags<I, S>(tags: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut bits = [0u64; TAG_WORDS];
        for tag in tags {
            let tag = tag.as_ref();
            let Some(tag_id) = tag_id(tag) else {
                debug_assert!(false, "unknown tag: {tag}");
                continue;
            };
            bits[tag_id / 64] |= 1u64 << (tag_id % 64);
        }

        Self { bits }
    }

    pub const fn insert(&mut self, tag_id: u16) {
        let tag_id = tag_id as usize;
        assert!(tag_id < tags::ALL_TAGS.len(), "tag id out of range");
        self.bits[tag_id / 64] |= 1u64 << (tag_id % 64);
    }

    /// Returns `true` if the two sets do not share any tag.
    pub fn is_disjoint(&self, other: &TagSet) -> bool {
        for idx in 0..TAG_WORDS {
            if (self.bits[idx] & other.bits[idx]) != 0 {
                return false;
            }
        }
        true
    }

    /// Returns `true` if all tags in `self` are also present in `other`.
    pub fn is_subset(&self, other: &TagSet) -> bool {
        for idx in 0..TAG_WORDS {
            if (self.bits[idx] & !other.bits[idx]) != 0 {
                return false;
            }
        }
        true
    }

    /// Iterates tags in deterministic id order.
    pub fn iter(&self) -> TagSetIter<'_> {
        TagSetIter {
            bits: &self.bits,
            word_idx: 0,
            cur_word: 0,
        }
    }

    /// Returns `true` if the set contains no tags.
    pub fn is_empty(&self) -> bool {
        self.bits.iter().all(|&w| w == 0)
    }
}

impl BitOrAssign<&TagSet> for TagSet {
    fn bitor_assign(&mut self, rhs: &TagSet) {
        for idx in 0..TAG_WORDS {
            self.bits[idx] |= rhs.bits[idx];
        }
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for TagSet {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct TagSetVisitor;

        impl<'de> Visitor<'de> for TagSetVisitor {
            type Value = TagSet;

            fn expecting(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                formatter.write_str("a sequence of tag strings")
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let mut tags = TagSet::default();
                while let Some(tag) = seq.next_element::<Cow<str>>()? {
                    let Some(tag_id) = tag_id(&tag) else {
                        let msg = format!(
                            "Type tag `{tag}` is not recognized. Check for typos or upgrade prek to get new tags."
                        );
                        return Err(A::Error::custom(msg));
                    };
                    let tag_id = u16::try_from(tag_id)
                        .map_err(|_| A::Error::custom("tag id out of range"))?;
                    tags.insert(tag_id);
                }
                Ok(tags)
            }
        }

        deserializer.deserialize_seq(TagSetVisitor)
    }
}

#[cfg(feature = "schemars")]
impl schemars::JsonSchema for TagSet {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> Cow<'static, str> {
        Cow::Borrowed("TagSet")
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "array",
            "items": {
                "type": "string",
            },
            "uniqueItems": true,
        })
    }
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
        return Ok(tags::TAG_SET_DIRECTORY);
    } else if metadata.is_symlink() {
        return Ok(tags::TAG_SET_SYMLINK);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        let file_type = metadata.file_type();
        if file_type.is_socket() {
            return Ok(tags::TAG_SET_SOCKET);
        }
    };

    let mut tags = tags::TAG_SET_FILE;

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
        tags.insert(tags::TAG_EXECUTABLE);
    } else {
        tags.insert(tags::TAG_NON_EXECUTABLE);
    }

    let filename_tags = tags_from_filename(path);
    tags |= &filename_tags;
    if executable {
        if let Ok(shebang) = parse_shebang(path) {
            let interpreter_tags = tags_from_interpreter(shebang[0].as_str());
            tags |= &interpreter_tags;
        }
    }

    if tags.is_disjoint(&tags::TAG_SET_TEXT_OR_BINARY) {
        if is_text_file(path) {
            tags.insert(tags::TAG_TEXT);
        } else {
            tags.insert(tags::TAG_BINARY);
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

    let mut result = TagSet::empty();

    if let Some(tags) = tags::NAMES.get(filename) {
        result |= tags;
    }
    if result.is_empty() {
        // # Allow e.g. "Dockerfile.xenial" to match "Dockerfile".
        if let Some(name) = filename.split('.').next() {
            if let Some(tags) = tags::NAMES.get(name) {
                result |= tags;
            }
        }
    }

    if let Some(ext) = ext {
        // Check if extension is already lowercase to avoid allocation
        if ext.chars().all(|c| c.is_ascii_lowercase()) {
            if let Some(tags) = tags::EXTENSIONS.get(ext) {
                result |= tags;
            }
        } else {
            let ext_lower = ext.to_ascii_lowercase();
            if let Some(tags) = tags::EXTENSIONS.get(ext_lower.as_str()) {
                result |= tags;
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
            return *tags;
        }

        // python3.12.3 should match python3.12.3, python3.12, python3, python
        if let Some(pos) = name.rfind('.') {
            name = &name[..pos];
        } else {
            break;
        }
    }

    TagSet::empty()
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
    use super::{TagSet, tags};
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
    fn tagset_new_iter_and_is_empty() {
        let empty = TagSet::new(&[]);
        assert!(empty.is_empty());
        assert_eq!(empty.iter().count(), 0);

        let binary_id = u16::try_from(super::tag_id("binary").expect("binary id")).unwrap();
        let text_id = u16::try_from(super::tag_id("text").expect("text id")).unwrap();
        let set = TagSet::new(&[text_id, binary_id, text_id]);

        assert!(!set.is_empty());
        assert_eq!(set.iter().collect::<Vec<_>>(), vec!["binary", "text"]);
    }

    #[test]
    fn tagset_from_tags_intersects_subset_and_bitor_assign() {
        let a = TagSet::from_tags(["python", "text"]);
        let b = TagSet::from_tags(["python"]);
        let c = TagSet::from_tags(["binary"]);

        assert!(b.is_subset(&a));
        assert!(!a.is_subset(&b));
        assert!(!a.is_disjoint(&b));
        assert!(a.is_disjoint(&c));

        let mut merged = b;
        merged |= &c;
        assert_tagset(&merged, &["python", "binary"]);
    }

    #[test]
    fn tagset_new_panics_on_out_of_range_id() {
        let out_of_range = u16::try_from(tags::ALL_TAGS.len()).unwrap();
        let result = std::panic::catch_unwind(|| TagSet::new(&[out_of_range]));
        assert!(result.is_err());
    }

    #[cfg(feature = "serde")]
    #[test]
    fn tagset_deserialize_from_string_slice() {
        let parsed: TagSet =
            serde_json::from_str(r#"["python","text"]"#).expect("should parse tags");
        assert_tagset(&parsed, &["python", "text"]);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn tagset_deserialize_unknown_tag_errors() {
        let err = serde_json::from_str::<TagSet>(r#"["not-a-real-tag"]"#).unwrap_err();
        assert!(
            err.to_string()
                .contains("Type tag `not-a-real-tag` is not recognized"),
            "unexpected error: {err}"
        );
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
