//! Persistent ignore matchers shared by directory traversal workers.

use std::{
    ffi::OsStr,
    fs::{self, File, FileType},
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    Error, Match, PartialErrorBuilder,
    gitignore::{self, Gitignore, GitignoreBuilder},
    pathutil::strip_prefix,
};

#[derive(Clone, Debug)]
pub(crate) struct Ignore {
    inner: Arc<IgnoreInner>,
    // Parent rules use the canonical root, while callbacks retain the caller's path.
    absolute_base: Option<Arc<Path>>,
}

#[derive(Debug)]
struct IgnoreInner {
    dir: PathBuf,
    parent: Option<Arc<Self>>,
    is_absolute_parent: bool,
    has_absolute_parent_rules: bool,
    custom_ignore_filename: Arc<OsStr>,
    custom_ignore_matcher: Gitignore,
    ignore_matcher: Gitignore,
    git_global_matcher: Arc<Gitignore>,
    git_ignore_matcher: Gitignore,
    git_exclude_matcher: Gitignore,
    has_git: bool,
}

impl Ignore {
    pub(crate) fn new(root: &Path, custom_ignore_filename: &str) -> Self {
        let global = match std::env::current_dir() {
            Ok(cwd) => {
                let (matcher, error) = GitignoreBuilder::new(cwd).build_global();
                if let Some(error) = error {
                    log::debug!("{error}");
                }
                matcher
            }
            Err(_) => Gitignore::empty(),
        };
        let mut ignore = Self {
            inner: Arc::new(IgnoreInner {
                dir: PathBuf::new(),
                parent: None,
                is_absolute_parent: true,
                has_absolute_parent_rules: false,
                custom_ignore_filename: Arc::from(OsStr::new(custom_ignore_filename)),
                custom_ignore_matcher: Gitignore::empty(),
                ignore_matcher: Gitignore::empty(),
                git_global_matcher: Arc::new(global),
                git_ignore_matcher: Gitignore::empty(),
                git_exclude_matcher: Gitignore::empty(),
                has_git: false,
            }),
            absolute_base: None,
        };
        let Ok(base) = root.canonicalize() else {
            return ignore;
        };
        ignore.absolute_base = Some(Arc::from(base.as_path()));
        let parents: Vec<_> = base.ancestors().skip(1).collect();
        for parent in parents.into_iter().rev() {
            let (mut inner, error) = ignore.add_child_inner(parent, None);
            if let Some(error) = error {
                log::debug!("{error}");
            }
            inner.is_absolute_parent = true;
            inner.has_absolute_parent_rules |= !inner.custom_ignore_matcher.is_empty()
                || !inner.ignore_matcher.is_empty()
                || !inner.git_ignore_matcher.is_empty()
                || !inner.git_exclude_matcher.is_empty();
            ignore.inner = Arc::new(inner);
        }
        ignore
    }

    /// A partial listing cannot prove that an ignore file is absent.
    pub(crate) fn add_child(&self, path: &Path, entries: Option<&[fs::DirEntry]>) -> Self {
        let files = entries
            .map(|entries| IgnoreFilesFound::new(entries, &self.inner.custom_ignore_filename));
        let (inner, error) = self.add_child_inner(path, files.as_ref());
        if let Some(error) = error {
            log::debug!("{error}");
        }
        Self {
            inner: Arc::new(inner),
            absolute_base: self.absolute_base.clone(),
        }
    }

    fn add_child_inner(
        &self,
        dir: &Path,
        files: Option<&IgnoreFilesFound>,
    ) -> (IgnoreInner, Option<Error>) {
        let git_type = if files.is_none_or(|files| files.git) {
            dir.join(".git")
                .metadata()
                .ok()
                .map(|metadata| metadata.file_type())
        } else {
            None
        };
        let has_git =
            git_type.is_some() || (files.is_none_or(|files| files.jj) && dir.join(".jj").exists());
        let mut errors = PartialErrorBuilder::default();
        let mut load = |present: bool, base: &Path, name: &OsStr| {
            if !present {
                return Gitignore::empty();
            }
            let (matcher, error) = create_gitignore(dir, &base.join(name));
            errors.maybe_push(error);
            matcher
        };
        let custom_ignore_matcher = load(
            files.is_none_or(|files| files.custom),
            dir,
            &self.inner.custom_ignore_filename,
        );
        let ignore_matcher = load(
            files.is_none_or(|files| files.ignore),
            dir,
            OsStr::new(".ignore"),
        );
        let git_ignore_matcher = load(
            files.is_none_or(|files| files.git_ignore),
            dir,
            OsStr::new(".gitignore"),
        );
        let git_exclude_matcher = if files.is_none_or(|files| files.git) {
            match resolve_git_commondir(dir, git_type) {
                Ok(git_dir) => load(true, &git_dir, OsStr::new("info/exclude")),
                Err(error) => {
                    errors.maybe_push(error);
                    Gitignore::empty()
                }
            }
        } else {
            Gitignore::empty()
        };
        (
            IgnoreInner {
                dir: dir.to_path_buf(),
                parent: Some(self.inner.clone()),
                is_absolute_parent: false,
                has_absolute_parent_rules: self.inner.has_absolute_parent_rules,
                custom_ignore_filename: self.inner.custom_ignore_filename.clone(),
                custom_ignore_matcher,
                ignore_matcher,
                git_global_matcher: self.inner.git_global_matcher.clone(),
                git_ignore_matcher,
                git_exclude_matcher,
                has_git,
            },
            errors.into_error_option(),
        )
    }

    pub(crate) fn matched(&self, path: &Path) -> Match<&gitignore::Glob> {
        self.matched_ignore(strip_prefix("./", path).unwrap_or(path), true)
    }

    fn parents(&self) -> impl Iterator<Item = &IgnoreInner> {
        std::iter::successors(Some(self.inner.as_ref()), |ignore| ignore.parent.as_deref())
    }
}

#[derive(Default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "Each flag independently records the presence of an ignore file or repository marker"
)]
struct IgnoreFilesFound {
    ignore: bool,
    git_ignore: bool,
    git: bool,
    jj: bool,
    custom: bool,
}

impl IgnoreFilesFound {
    fn new(entries: &[fs::DirEntry], custom: &OsStr) -> Self {
        let mut found = Self::default();
        for entry in entries {
            let name = entry.file_name();
            found.ignore |= name == ".ignore";
            found.git_ignore |= name == ".gitignore";
            found.git |= name == ".git";
            found.jj |= name == ".jj";
            found.custom |= name == custom;
        }
        found
    }
}

fn create_gitignore(root: &Path, path: &Path) -> (Gitignore, Option<Error>) {
    let mut builder = GitignoreBuilder::new(root);
    let mut errors = PartialErrorBuilder::default();
    // Avoid opening absent ancestor ignore files. Windows file probes can cost
    // more than attempting the open, so preserve upstream's platform choice.
    if cfg!(windows) || path.exists() {
        errors.maybe_push_ignore_io(builder.add(path));
    }
    let matcher = match builder.build() {
        Ok(matcher) => matcher,
        Err(error) => {
            errors.push(error);
            Gitignore::empty()
        }
    };
    (matcher, errors.into_error_option())
}

impl Ignore {
    /// Performs matching only on the ignore files for this directory and
    /// all parent directories.
    fn matched_ignore<'a>(&'a self, path: &Path, is_dir: bool) -> Match<&'a gitignore::Glob> {
        let (mut m_custom_ignore, mut m_ignore, mut m_gi, mut m_gi_exclude) =
            (Match::None, Match::None, Match::None, Match::None);
        let any_git = self.parents().any(|ig| ig.has_git);
        let mut saw_git = false;
        for ig in self.parents().take_while(|ig| !ig.is_absolute_parent) {
            if m_custom_ignore.is_none() {
                m_custom_ignore = ig.custom_ignore_matcher.matched(path, is_dir);
            }
            if m_ignore.is_none() {
                m_ignore = ig.ignore_matcher.matched(path, is_dir);
            }
            if any_git && !saw_git && m_gi.is_none() {
                m_gi = ig.git_ignore_matcher.matched(path, is_dir);
            }
            if any_git && !saw_git && m_gi_exclude.is_none() {
                m_gi_exclude = ig.git_exclude_matcher.matched(path, is_dir);
            }
            saw_git = saw_git || ig.has_git;
        }
        if self.inner.has_absolute_parent_rules {
            if let Some(abs_parent_path) = self.absolute_base.as_deref() {
                // Parent matchers use canonical paths. Rewrite from the walk
                // root so relative or symlinked roots do not duplicate prefixes.
                let path = abs_parent_path.join(
                    self.parents()
                        .take_while(|ig| !ig.is_absolute_parent)
                        .last()
                        .map_or(path, |ig| {
                            // Stripping a root of "." would also strip the
                            // leading dot from a hidden child filename.
                            if ig.dir.as_path() == Path::new(".") {
                                return path;
                            }
                            let without_dot_slash = strip_if_is_prefix("./", ig.dir.as_path());
                            let relative_base = strip_if_is_prefix(without_dot_slash, path);
                            strip_if_is_prefix("/", relative_base)
                        }),
                );

                for ig in self.parents().skip_while(|ig| !ig.is_absolute_parent) {
                    if m_custom_ignore.is_none() {
                        m_custom_ignore = ig.custom_ignore_matcher.matched(&path, is_dir);
                    }
                    if m_ignore.is_none() {
                        m_ignore = ig.ignore_matcher.matched(&path, is_dir);
                    }
                    if any_git && !saw_git && m_gi.is_none() {
                        m_gi = ig.git_ignore_matcher.matched(&path, is_dir);
                    }
                    if any_git && !saw_git && m_gi_exclude.is_none() {
                        m_gi_exclude = ig.git_exclude_matcher.matched(&path, is_dir);
                    }
                    saw_git = saw_git || ig.has_git;
                }
            }
        }
        let m_global = if any_git {
            self.inner.git_global_matcher.matched(path, is_dir)
        } else {
            Match::None
        };

        m_custom_ignore
            .or(m_ignore)
            .or(m_gi)
            .or(m_gi_exclude)
            .or(m_global)
    }
}
/// Find the `GIT_COMMON_DIR` for the given git worktree.
///
/// This is the directory that may contain a private ignore file
/// "info/exclude". Unlike git, this function does *not* read environment
/// variables `GIT_DIR` and `GIT_COMMON_DIR`, because it is not clear how to use
/// them when multiple repositories are searched.
///
/// Some I/O errors are ignored.
fn resolve_git_commondir(dir: &Path, git_type: Option<FileType>) -> Result<PathBuf, Option<Error>> {
    let git_dir_path = || dir.join(".git");
    let git_dir = git_dir_path();
    if !git_type.is_some_and(|ft| ft.is_file()) {
        return Ok(git_dir);
    }
    let file = match File::open(git_dir) {
        Ok(file) => io::BufReader::new(file),
        Err(err) => {
            return Err(Some(Error::Io(err).with_path(git_dir_path())));
        }
    };
    let dot_git_line = match file.lines().next() {
        Some(Ok(line)) => line,
        Some(Err(err)) => {
            return Err(Some(Error::Io(err).with_path(git_dir_path())));
        }
        None => return Err(None),
    };
    if !dot_git_line.starts_with("gitdir: ") {
        return Err(None);
    }
    let real_git_dir = PathBuf::from(&dot_git_line["gitdir: ".len()..]);
    let git_commondir_file = || real_git_dir.join("commondir");
    let file = match File::open(git_commondir_file()) {
        Ok(file) => io::BufReader::new(file),
        Err(_) => return Err(None),
    };
    let commondir_line = match file.lines().next() {
        Some(Ok(line)) => line,
        Some(Err(err)) => {
            return Err(Some(Error::Io(err).with_path(git_commondir_file())));
        }
        None => return Err(None),
    };
    let commondir_abs = if commondir_line.starts_with('.') {
        real_git_dir.join(commondir_line) // relative commondir
    } else {
        PathBuf::from(commondir_line)
    };
    Ok(commondir_abs)
}

/// Strips `prefix` from `path` if it's a prefix, otherwise returns `path`
/// unchanged.
fn strip_if_is_prefix<'a, P: AsRef<Path> + ?Sized>(prefix: &'a P, path: &'a Path) -> &'a Path {
    strip_prefix(prefix, path).map_or(path, |p| p)
}
