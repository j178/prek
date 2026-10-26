/*!
The gitignore module provides a way to match globs from a gitignore file
against file paths.

Note that this module implements the specification as described in the
`gitignore` man page from scratch. That is, this module does *not* shell out to
the `git` command line tool.
*/

use std::{
    fs::File,
    io::{BufRead, BufReader, Read},
    path::{Path, PathBuf},
};

use globset::{Candidate, GlobBuilder, GlobSet, GlobSetBuilder};

use crate::{
    Error, Match, PartialErrorBuilder,
    pathutil::{is_file_name, strip_prefix},
};

/// Gitignore is a matcher for the globs in one or more gitignore files
/// in the same directory.
#[derive(Debug)]
pub struct Gitignore {
    set: GlobSet,
    root: PathBuf,
    whitelists: Vec<bool>,
}

impl Gitignore {
    /// Creates a new empty gitignore matcher that never matches anything.
    ///
    /// Its path is empty.
    pub fn empty() -> Gitignore {
        Gitignore {
            set: GlobSet::empty(),
            root: PathBuf::from(""),
            whitelists: vec![],
        }
    }

    /// Returns true if and only if this gitignore has zero globs, and
    /// therefore never matches any file path.
    pub fn is_empty(&self) -> bool {
        self.set.is_empty()
    }

    /// Matches a directory, using the last matching rule in this ignore file.
    pub fn matched(&self, path: &Path, matches: &mut Vec<usize>) -> Match {
        if self.is_empty() {
            return Match::None;
        }
        self.set
            .matches_candidate_into(&Candidate::new(self.strip(path)), matches);
        match matches.last() {
            Some(&index) if self.whitelists[index] => Match::Whitelist,
            Some(_) => Match::Ignore,
            None => Match::None,
        }
    }

    /// Strips the given path such that it's suitable for matching with this
    /// gitignore matcher.
    fn strip<'a, P: 'a + AsRef<Path> + ?Sized>(&'a self, path: &'a P) -> &'a Path {
        let mut path = path.as_ref();
        // A leading ./ is completely superfluous. We also strip it from
        // our gitignore root path, so we need to strip it from our candidate
        // path too.
        if let Some(p) = strip_prefix("./", path) {
            path = p;
        }
        // Strip any common prefix between the candidate path and the root
        // of the gitignore, to make sure we get relative matching right.
        // BUT, a file name might not have any directory components to it,
        // in which case, we don't want to accidentally strip any part of the
        // file name.
        //
        // As an additional special case, if the root is just `.`, then we
        // shouldn't try to strip anything, e.g., when path begins with a `.`.
        if self.root != Path::new(".") && !is_file_name(path) {
            if let Some(p) = strip_prefix(&self.root, path) {
                path = p;
                // If we're left with a leading slash, get rid of it.
                if let Some(p) = strip_prefix("/", path) {
                    path = p;
                }
            }
        }
        path
    }
}

/// Builds a matcher for a single set of globs from a .gitignore file.
#[derive(Debug)]
pub struct GitignoreBuilder {
    builder: GlobSetBuilder,
    root: PathBuf,
    whitelists: Vec<bool>,
}

impl GitignoreBuilder {
    /// Create a new builder for a gitignore file.
    ///
    /// The path given should be the path at which the globs for this gitignore
    /// file should be matched. Note that paths are always matched relative
    /// to the root path given here. Generally, the root path should correspond
    /// to the *directory* containing a `.gitignore` file.
    pub fn new<P: AsRef<Path>>(root: P) -> GitignoreBuilder {
        let root = root.as_ref();
        GitignoreBuilder {
            builder: GlobSetBuilder::new(),
            root: strip_prefix("./", root).unwrap_or(root).to_path_buf(),
            whitelists: vec![],
        }
    }

    /// Builds a new matcher from the globs added so far.
    ///
    /// Once a matcher is built, no new globs can be added to it.
    pub fn build(self) -> Result<Gitignore, Error> {
        if self.whitelists.is_empty() {
            return Ok(Gitignore::empty());
        }
        let set = self.builder.build().map_err(|err| Error::Glob {
            glob: None,
            err: err.to_string(),
        })?;
        Ok(Gitignore {
            set,
            root: self.root,
            whitelists: self.whitelists,
        })
    }

    /// Loads Git's global ignore file, retaining this builder's matching root.
    pub fn build_global(mut self) -> (Gitignore, Option<Error>) {
        match gitconfig_excludes_path() {
            None => (Gitignore::empty(), None),
            Some(path) => {
                if !path.is_file() {
                    (Gitignore::empty(), None)
                } else {
                    let mut errs = PartialErrorBuilder::default();
                    errs.maybe_push_ignore_io(self.add(path));
                    match self.build() {
                        Ok(gi) => (gi, errs.into_error_option()),
                        Err(err) => {
                            errs.push(err);
                            (Gitignore::empty(), errs.into_error_option())
                        }
                    }
                }
            }
        }
    }

    /// Add each glob from the file path given.
    ///
    /// The file given should be formatted as a `gitignore` file.
    ///
    /// Note that partial errors can be returned. For example, if there was
    /// a problem adding one glob, an error for that will be returned, but
    /// all other valid globs will still be added.
    pub fn add<P: AsRef<Path>>(&mut self, path: P) -> Option<Error> {
        let path = path.as_ref();
        let file = match File::open(path) {
            Err(err) => return Some(Error::Io(err).with_path(path)),
            Ok(file) => file,
        };
        tracing::debug!("opened gitignore file: {}", path.display());
        let rdr = BufReader::new(file);
        let mut errs = PartialErrorBuilder::default();
        for (i, line) in rdr.lines().enumerate() {
            let lineno = (i + 1) as u64;
            let line = match line {
                Ok(line) => line,
                Err(err) => {
                    errs.push(Error::Io(err).tagged(path, lineno));
                    break;
                }
            };

            // Match Git's handling of .gitignore files that begin with the Unicode BOM
            const UTF8_BOM: &str = "\u{feff}";
            let line = if i == 0 {
                line.trim_start_matches(UTF8_BOM)
            } else {
                &line
            };

            if let Err(err) = self.add_line(line) {
                errs.push(err.tagged(path, lineno));
            }
        }
        errs.into_error_option()
    }

    /// Add a line from a gitignore file to this builder.
    ///
    /// If the line could not be parsed as a glob, then an error is returned.
    fn add_line(&mut self, mut line: &str) -> Result<&mut GitignoreBuilder, Error> {
        if line.starts_with('#') {
            return Ok(self);
        }
        if !line.ends_with("\\ ") {
            line = line.trim_end();
        }
        if line.is_empty() {
            return Ok(self);
        }
        let original = line;
        let mut is_whitelist = false;
        let mut is_absolute = false;
        if line.starts_with("\\!") || line.starts_with("\\#") {
            line = &line[1..];
            is_absolute = line.chars().nth(0) == Some('/');
        } else {
            if line.starts_with('!') {
                is_whitelist = true;
                line = &line[1..];
            }
            if line.starts_with('/') {
                // `man gitignore` says that if a glob starts with a slash,
                // then the glob can only match the beginning of a path
                // (relative to the location of gitignore). We achieve this by
                // simply banning wildcards from matching /.
                line = &line[1..];
                is_absolute = true;
            }
        }
        // All candidates are directories, so a trailing slash only affects parsing.
        if line.as_bytes().last() == Some(&b'/') {
            line = &line[..line.len() - 1];
            // If the slash was escaped, then remove the escape.
            // See: https://github.com/BurntSushi/ripgrep/issues/2236
            if line.as_bytes().last() == Some(&b'\\') {
                line = &line[..line.len() - 1];
            }
        }
        let mut actual = line.to_string();
        // If there is a literal slash, then this is a glob that must match the
        // entire path name. Otherwise, we should let it match anywhere, so use
        // a **/ prefix.
        if !is_absolute && !line.chars().any(|c| c == '/') {
            // ... but only if we don't already have a **/ prefix.
            if !(actual.starts_with("**/") || actual == "**") {
                actual = format!("**/{actual}");
            }
        }
        // If the glob ends with `/**`, then we should only match everything
        // inside a directory, but not the directory itself. Standard globs
        // will match the directory. So we add `/*` to force the issue.
        if actual.ends_with("/**") {
            actual = format!("{actual}/*");
        }
        let parsed = GlobBuilder::new(&actual)
            .literal_separator(true)
            .case_insensitive(false)
            .backslash_escape(true)
            .allow_unclosed_class(true)
            .build()
            .map_err(|err| Error::Glob {
                glob: Some(original.to_string()),
                err: err.kind().to_string(),
            })?;
        self.builder.add(parsed);
        self.whitelists.push(is_whitelist);
        Ok(self)
    }
}

/// Return the file path of the current environment's global gitignore file.
///
/// Note that the file path returned may not exist.
pub fn gitconfig_excludes_path() -> Option<PathBuf> {
    // When GIT_CONFIG_GLOBAL is set, it replaces both $HOME/.gitconfig and
    // $XDG_CONFIG_HOME/git/config (per git 2.32+). Otherwise, git supports
    // $HOME/.gitconfig and $XDG_CONFIG_HOME/git/config simultaneously, where
    // $HOME/.gitconfig takes precedent.
    gitconfig_global_env_contents()
        .and_then(|x| parse_excludes_file(&x))
        .or_else(|| gitconfig_home_contents().and_then(|x| parse_excludes_file(&x)))
        .or_else(|| gitconfig_xdg_contents().and_then(|x| parse_excludes_file(&x)))
        // System-level config has the lowest priority for core.excludesFile.
        // GIT_CONFIG_SYSTEM overrides the default /etc/gitconfig path.
        .or_else(|| gitconfig_system_contents().and_then(|x| parse_excludes_file(&x)))
        .or_else(excludes_file_default)
}

/// Returns the file contents of git's global config file from the path
/// specified by the `GIT_CONFIG_GLOBAL` environment variable.
#[expect(
    clippy::disallowed_methods,
    reason = "Preserve upstream Git and XDG environment handling in this vendored matcher"
)]
fn gitconfig_global_env_contents() -> Option<Vec<u8>> {
    let path = std::env::var_os("GIT_CONFIG_GLOBAL").map(PathBuf::from)?;
    if path.as_os_str().is_empty() {
        return None;
    }
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut contents = vec![];
    file.read_to_end(&mut contents).ok().map(|_| contents)
}

/// Returns the file contents of git's system-level config file.
///
/// Checks `GIT_CONFIG_SYSTEM` first, then falls back to `/etc/gitconfig`.
#[expect(
    clippy::disallowed_methods,
    reason = "Preserve upstream Git and XDG environment handling in this vendored matcher"
)]
fn gitconfig_system_contents() -> Option<Vec<u8>> {
    let path = std::env::var_os("GIT_CONFIG_SYSTEM")
        .map(PathBuf::from)
        .filter(|x| !x.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/etc/gitconfig"));
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut contents = vec![];
    file.read_to_end(&mut contents).ok().map(|_| contents)
}

/// Returns the file contents of git's global config file, if one exists, in
/// the user's home directory.
fn gitconfig_home_contents() -> Option<Vec<u8>> {
    let home = home_dir()?;
    let mut file = BufReader::new(File::open(home.join(".gitconfig")).ok()?);
    let mut contents = vec![];
    file.read_to_end(&mut contents).ok().map(|_| contents)
}

/// Returns the file contents of git's global config file, if one exists, in
/// the user's `XDG_CONFIG_HOME` directory.
#[expect(
    clippy::disallowed_methods,
    reason = "Preserve upstream Git and XDG environment handling in this vendored matcher"
)]
fn gitconfig_xdg_contents() -> Option<Vec<u8>> {
    let path = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|x| !x.as_os_str().is_empty())
        .or_else(|| home_dir().map(|p| p.join(".config")))
        .map(|x| x.join("git/config"))?;
    let mut file = BufReader::new(File::open(path).ok()?);
    let mut contents = vec![];
    file.read_to_end(&mut contents).ok().map(|_| contents)
}

/// Returns the default file path for a global .gitignore file.
///
/// Specifically, this respects `XDG_CONFIG_HOME`.
#[expect(
    clippy::disallowed_methods,
    reason = "Preserve upstream Git and XDG environment handling in this vendored matcher"
)]
fn excludes_file_default() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|x| !x.as_os_str().is_empty())
        .or_else(|| home_dir().map(|p| p.join(".config")))
        .map(|x| x.join("git/ignore"))
}

/// Extract git's `core.excludesfile` config setting from the raw file contents
/// given.
fn parse_excludes_file(data: &[u8]) -> Option<PathBuf> {
    use std::sync::OnceLock;

    use regex::bytes::Regex;

    // Preserve upstream's permissive lookup of quoted and unquoted paths
    // without introducing a full Git configuration parser.
    static RE: OnceLock<Option<Regex>> = OnceLock::new();
    let re = RE
        .get_or_init(|| Regex::new(r#"(?im-u)^\s*excludesfile\s*=\s*"?\s*(\S+?)\s*"?\s*$"#).ok())
        .as_ref()?;
    let captures = re.captures(data)?;
    let candidate = captures.get(1)?.as_bytes();
    std::str::from_utf8(candidate)
        .ok()
        .map(|s| PathBuf::from(expand_tilde(s)))
}

/// Expands ~ in file paths to the value of $HOME.
fn expand_tilde(path: &str) -> String {
    let home = match home_dir() {
        None => return path.to_string(),
        Some(home) => home.to_string_lossy().into_owned(),
    };
    path.replace('~', &home)
}

/// Returns the location of the user's home directory.
fn home_dir() -> Option<PathBuf> {
    // We're fine with using std::env::home_dir for now. Its bugs are, IMO,
    // pretty minor corner cases.
    #![allow(deprecated)]
    std::env::home_dir()
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::{Gitignore, GitignoreBuilder};

    fn gi_from_str<P: AsRef<Path>>(root: P, s: &str) -> Gitignore {
        let mut builder = GitignoreBuilder::new(root);
        for line in s.lines() {
            builder.add_line(line).unwrap();
        }
        builder.build().unwrap()
    }

    macro_rules! ignored {
        ($name:ident, $root:expr, $gi:expr, $path:expr) => {
            #[doc = concat!("Checks the ignore pattern ", stringify!($gi), " against ", stringify!($path), ".")]
            #[test]
            fn $name() {
                let gi = gi_from_str($root, $gi);
                assert!(gi.matched(Path::new($path), &mut Vec::new()).is_ignore());
            }
        };
    }

    macro_rules! not_ignored {
        ($name:ident, $root:expr, $gi:expr, $path:expr) => {
            #[doc = concat!("Checks the ignore pattern ", stringify!($gi), " against ", stringify!($path), ".")]
            #[test]
            fn $name() {
                let gi = gi_from_str($root, $gi);
                assert!(!gi.matched(Path::new($path), &mut Vec::new()).is_ignore());
            }
        };
    }

    const ROOT: &str = "/home/foobar/rust/rg";

    ignored!(ig1, ROOT, "months", "months");
    ignored!(ig2, ROOT, "*.lock", "Cargo.lock");
    ignored!(ig3, ROOT, "*.rs", "src/main.rs");
    ignored!(ig4, ROOT, "src/*.rs", "src/main.rs");
    ignored!(ig5, ROOT, "/*.c", "cat-file.c");
    ignored!(ig6, ROOT, "/src/*.rs", "src/main.rs");
    ignored!(ig7, ROOT, "!src/main.rs\n*.rs", "src/main.rs");
    ignored!(ig8, ROOT, "foo/", "foo");
    ignored!(ig9, ROOT, "**/foo", "foo");
    ignored!(ig10, ROOT, "**/foo", "src/foo");
    ignored!(ig11, ROOT, "**/foo/**", "src/foo/bar");
    ignored!(ig12, ROOT, "**/foo/**", "wat/src/foo/bar/baz");
    ignored!(ig13, ROOT, "**/foo/bar", "foo/bar");
    ignored!(ig14, ROOT, "**/foo/bar", "src/foo/bar");
    ignored!(ig15, ROOT, "abc/**", "abc/x");
    ignored!(ig16, ROOT, "abc/**", "abc/x/y");
    ignored!(ig17, ROOT, "abc/**", "abc/x/y/z");
    ignored!(ig18, ROOT, "a/**/b", "a/b");
    ignored!(ig19, ROOT, "a/**/b", "a/x/b");
    ignored!(ig20, ROOT, "a/**/b", "a/x/y/b");
    ignored!(ig21, ROOT, r"\!xy", "!xy");
    ignored!(ig22, ROOT, r"\#foo", "#foo");
    ignored!(ig23, ROOT, "foo", "./foo");
    ignored!(ig24, ROOT, "target", "grep/target");
    ignored!(ig25, ROOT, "Cargo.lock", "./tabwriter-bin/Cargo.lock");
    ignored!(ig26, ROOT, "/foo/bar/baz", "./foo/bar/baz");
    ignored!(ig27, ROOT, "foo/", "xyz/foo");
    ignored!(ig28, "./src", "/llvm/", "./src/llvm");
    ignored!(ig29, ROOT, "node_modules/ ", "node_modules");
    ignored!(ig30, ROOT, "**/", "foo/bar");
    ignored!(ig31, ROOT, "path1/*", "path1/foo");
    ignored!(ig32, ROOT, ".a/b", ".a/b");
    ignored!(ig33, "./", ".a/b", ".a/b");
    ignored!(ig34, ".", ".a/b", ".a/b");
    ignored!(ig35, "./.", ".a/b", ".a/b");
    ignored!(ig36, "././", ".a/b", ".a/b");
    ignored!(ig37, "././.", ".a/b", ".a/b");
    ignored!(ig38, ROOT, "\\[", "[");
    ignored!(ig39, ROOT, "\\?", "?");
    ignored!(ig40, ROOT, "\\*", "*");
    ignored!(ig41, ROOT, "\\a", "a");
    ignored!(ig42, ROOT, "s*.rs", "sfoo.rs");
    ignored!(ig43, ROOT, "**", "foo.rs");
    ignored!(ig44, ROOT, "**/**/*", "a/foo.rs");

    not_ignored!(ignot1, ROOT, "amonths", "months");
    not_ignored!(ignot2, ROOT, "monthsa", "months");
    not_ignored!(ignot3, ROOT, "/src/*.rs", "src/grep/src/main.rs");
    not_ignored!(ignot4, ROOT, "/*.c", "mozilla-sha1/sha1.c");
    not_ignored!(ignot5, ROOT, "/src/*.rs", "src/grep/src/main.rs");
    not_ignored!(ignot6, ROOT, "*.rs\n!src/main.rs", "src/main.rs");
    not_ignored!(ignot8, ROOT, "**/foo/**", "wat/src/afoo/bar/baz");
    not_ignored!(ignot9, ROOT, "**/foo/**", "wat/src/fooa/bar/baz");
    not_ignored!(ignot10, ROOT, "**/foo/bar", "foo/src/bar");
    not_ignored!(ignot11, ROOT, "#foo", "#foo");
    not_ignored!(ignot12, ROOT, "\n\n\n", "foo");
    not_ignored!(ignot13, ROOT, "foo/**", "foo");
    not_ignored!(
        ignot14,
        "./third_party/protobuf",
        "m4/ltoptions.m4",
        "./third_party/protobuf/csharp/src/packages/repositories.config"
    );
    not_ignored!(ignot15, ROOT, "!/bar", "foo/bar");
    not_ignored!(ignot16, ROOT, "*\n!**/", "foo");
    not_ignored!(ignot17, ROOT, "src/*.rs", "src/grep/src/main.rs");
    not_ignored!(ignot18, ROOT, "path1/*", "path2/path1/foo");
    not_ignored!(ignot19, ROOT, "s*.rs", "src/foo.rs");

    fn bytes(s: &str) -> Vec<u8> {
        s.to_string().into_bytes()
    }

    fn path_string<P: AsRef<Path>>(path: P) -> String {
        path.as_ref().to_str().unwrap().to_string()
    }

    /// Parses an absolute core.excludesFile path.
    #[test]
    fn parse_excludes_file1() {
        let data = bytes("[core]\nexcludesFile = /foo/bar");
        let got = super::parse_excludes_file(&data).unwrap();
        assert_eq!(path_string(got), "/foo/bar");
    }

    /// Expands a home-relative excludes file path.
    #[test]
    fn parse_excludes_file2() {
        let data = bytes("[core]\nexcludesFile = ~/foo/bar");
        let got = super::parse_excludes_file(&data).unwrap();
        assert_eq!(path_string(got), super::expand_tilde("~/foo/bar"));
    }

    /// Does not confuse an unknown setting with core.excludesFile.
    #[test]
    fn parse_excludes_file3() {
        let data = bytes("[core]\nexcludeFile = /foo/bar");
        assert!(super::parse_excludes_file(&data).is_none());
    }

    /// Removes surrounding quotes before expanding a home-relative path.
    #[test]
    fn parse_excludes_file4() {
        let data = bytes("[core]\nexcludesFile = \"~/foo/bar\"");
        let got = super::parse_excludes_file(&data);
        assert_eq!(path_string(got.unwrap()), super::expand_tilde("~/foo/bar"));
    }

    /// Rejects malformed nested quotes in an excludes file path.
    #[test]
    fn parse_excludes_file5() {
        let data = bytes("[core]\nexcludesFile = \" \"~/foo/bar \" \"");
        assert!(super::parse_excludes_file(&data).is_none());
    }

    // See: https://github.com/BurntSushi/ripgrep/issues/106
    /// Whitespace-only ignore lines do not become invalid globs.
    #[test]
    fn regression_106() {
        gi_from_str("/", " ");
    }

    ignored!(cs1, ROOT, "*.html", "foo.html");
    not_ignored!(cs2, ROOT, "*.html", "foo.HTML");
    not_ignored!(cs3, ROOT, "*.html", "foo.htm");
    not_ignored!(cs4, ROOT, "*.html", "foo.HTM");
}
