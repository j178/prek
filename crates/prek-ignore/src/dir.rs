//! Persistent ignore rules shared by directory traversal workers.

use std::{
    ffi::{OsStr, OsString},
    fs::{self, File, FileType},
    io::{self, BufRead},
    path::{Path, PathBuf},
    sync::Arc,
};

use crate::{
    Error, Match, PartialErrorBuilder,
    gitignore::{Gitignore, GitignoreBuilder},
    pathutil::strip_prefix,
};

#[derive(Clone, Debug)]
pub(crate) struct Ignore {
    rules: Option<Arc<Rules>>,
    context: Arc<Context>,
}

#[derive(Debug)]
struct Context {
    root: PathBuf,
    canonical_root: Option<PathBuf>,
    custom_ignore_filename: OsString,
    global: Gitignore,
}

/// Only directories that introduce rules or a repository boundary need a node.
#[derive(Debug)]
struct Rules {
    parent: Option<Arc<Self>>,
    outside_root: bool,
    in_git: bool,
    repository: bool,
    custom: Gitignore,
    ignore: Gitignore,
    git_ignore: Gitignore,
    git_exclude: Gitignore,
}

impl Ignore {
    pub(crate) fn new(root: &Path, custom_ignore_filename: &str) -> Self {
        let global = match std::env::current_dir() {
            Ok(cwd) => {
                let (matcher, error) = GitignoreBuilder::new(cwd).build_global();
                if let Some(error) = error {
                    tracing::debug!("{error}");
                }
                matcher
            }
            Err(_) => Gitignore::empty(),
        };
        let canonical_root = root.canonicalize().ok();
        let context = Arc::new(Context {
            root: strip_prefix("./", root).unwrap_or(root).to_path_buf(),
            // Matching strips byte prefixes, so component-equivalent paths
            // containing `.` still need their canonical spelling.
            canonical_root: canonical_root
                .clone()
                .filter(|base| base.as_os_str() != root.as_os_str()),
            custom_ignore_filename: custom_ignore_filename.into(),
            global,
        });
        let mut ignore = Self {
            rules: None,
            context: context.clone(),
        };
        if let Some(base) = &canonical_root {
            let parents: Vec<_> = base.ancestors().skip(1).collect();
            for parent in parents.into_iter().rev() {
                ignore = ignore.add_rules(parent, None, true);
            }
        }
        ignore
    }

    /// A partial listing cannot prove that an ignore file is absent.
    pub(crate) fn add_child(&self, path: &Path, entries: Option<&[fs::DirEntry]>) -> Self {
        let files = entries
            .map(|entries| IgnoreFilesFound::new(entries, &self.context.custom_ignore_filename));
        self.add_rules(path, files.as_ref(), false)
    }

    fn add_rules(&self, dir: &Path, files: Option<&IgnoreFilesFound>, outside_root: bool) -> Self {
        let git_type = if files.is_none_or(|files| files.git) {
            dir.join(".git")
                .metadata()
                .ok()
                .map(|metadata| metadata.file_type())
        } else {
            None
        };
        let repository =
            git_type.is_some() || (files.is_none_or(|files| files.jj) && dir.join(".jj").exists());
        let load = |present: bool, base: &Path, name: &OsStr| {
            if !present {
                return Gitignore::empty();
            }
            let (matcher, error) = create_gitignore(dir, &base.join(name));
            if let Some(error) = error {
                tracing::debug!("{error}");
            }
            matcher
        };
        let custom = load(
            files.is_none_or(|files| files.custom),
            dir,
            &self.context.custom_ignore_filename,
        );
        let ignore = load(
            files.is_none_or(|files| files.ignore),
            dir,
            OsStr::new(".ignore"),
        );
        let git_ignore = load(
            files.is_none_or(|files| files.git_ignore),
            dir,
            OsStr::new(".gitignore"),
        );
        let git_exclude = if files.is_none_or(|files| files.git) {
            match resolve_git_commondir(dir, git_type) {
                Ok(git_dir) => load(true, &git_dir, OsStr::new("info/exclude")),
                Err(error) => {
                    if let Some(error) = error {
                        tracing::debug!("{error}");
                    }
                    Gitignore::empty()
                }
            }
        } else {
            Gitignore::empty()
        };
        if !repository
            && custom.is_empty()
            && ignore.is_empty()
            && git_ignore.is_empty()
            && git_exclude.is_empty()
        {
            return self.clone();
        }
        Self {
            rules: Some(Arc::new(Rules {
                parent: self.rules.clone(),
                outside_root,
                in_git: repository || self.rules.as_ref().is_some_and(|rules| rules.in_git),
                repository,
                custom,
                ignore,
                git_ignore,
                git_exclude,
            })),
            context: self.context.clone(),
        }
    }

    pub(crate) fn matched(&self, path: &Path, matches: &mut Vec<usize>) -> Match {
        let path = strip_prefix("./", path).unwrap_or(path);
        let in_git = self.rules.as_ref().is_some_and(|rules| rules.in_git);
        let mut saw_git = false;
        let (mut ignore, mut git_ignore, mut git_exclude) = (Match::None, Match::None, Match::None);
        let mut canonical_path = None;
        for rules in std::iter::successors(self.rules.as_deref(), |rules| rules.parent.as_deref()) {
            // Parent rules use the canonical root, while callbacks and rules
            // inside the walk retain the caller's spelling. The root belongs
            // to the walk, even when it has no rules of its own.
            let candidate = if rules.outside_root
                && let Some(root) = &self.context.canonical_root
            {
                canonical_path
                    .get_or_insert_with(|| {
                        root.join(path.strip_prefix(&self.context.root).unwrap_or(path))
                    })
                    .as_path()
            } else {
                path
            };
            let custom = rules.custom.matched(candidate, matches);
            if !custom.is_none() {
                return custom;
            }
            if ignore.is_none() {
                ignore = rules.ignore.matched(candidate, matches);
            }
            if in_git && !saw_git {
                if git_ignore.is_none() {
                    git_ignore = rules.git_ignore.matched(candidate, matches);
                }
                if git_exclude.is_none() {
                    git_exclude = rules.git_exclude.matched(candidate, matches);
                }
            }
            saw_git |= rules.repository;
        }
        let matched = ignore.or(git_ignore).or(git_exclude);
        if !matched.is_none() || !in_git {
            return matched;
        }
        self.context.global.matched(path, matches)
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
    fn new(entries: &[fs::DirEntry], custom_ignore_filename: &OsStr) -> Self {
        let mut found = Self::default();
        for entry in entries {
            let name = entry.file_name();
            found.custom |= name == custom_ignore_filename;
            match name.as_encoded_bytes() {
                b".ignore" => found.ignore = true,
                b".gitignore" => found.git_ignore = true,
                b".git" => found.git = true,
                b".jj" => found.jj = true,
                _ => {}
            }
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
