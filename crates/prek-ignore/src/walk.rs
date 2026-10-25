use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use rayon::Scope;

use crate::{dir::Ignore, pathutil::is_hidden_entry};

/// Controls traversal after visiting a directory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WalkState {
    /// Visit this directory's eligible children.
    Continue,
    /// Skip this directory's children.
    Skip,
    /// Stop the walk. Other callbacks already running may still complete.
    Quit,
}

/// Visits directories in parallel, respecting ignore files and repository boundaries.
///
/// Rules come from `custom_ignore_filename`, `.ignore`, `.gitignore`, Git's
/// `info/exclude` and global excludes, in that precedence order. Parent rules
/// apply as in upstream `ignore`. Hidden directories are skipped unless an
/// ignore rule explicitly includes them. Child symlinks are never followed.
///
/// Each callback runs before its children are visited. Its second argument
/// borrows the complete, unfiltered listing, including ordinary files, hidden
/// entries and ignored entries. `None` means the listing could not be read
/// completely. The root is visited even if it is hidden or unreadable.
///
/// Directory read failures and invalid ignore rules do not abort traversal.
/// Valid rules from partially invalid ignore files still apply. Callback order
/// is unspecified, and returning [`WalkState::Quit`] cancels asynchronously.
pub fn walk<F>(root: &Path, custom_ignore_filename: &str, visit: F)
where
    F: Fn(&Path, Option<&[fs::DirEntry]>) -> WalkState + Sync,
{
    if !root.is_dir() {
        return;
    }
    let walker = Walker {
        quit: AtomicBool::new(false),
        visit,
    };
    let work = Work {
        path: root.to_path_buf(),
        ignore: Ignore::new(root, custom_ignore_filename),
    };
    rayon::in_place_scope(|scope| walker.run(scope, &work));
}

struct Work {
    path: PathBuf,
    ignore: Ignore,
}

struct Walker<F> {
    quit: AtomicBool,
    visit: F,
}

impl<F> Walker<F>
where
    F: Fn(&Path, Option<&[fs::DirEntry]>) -> WalkState + Sync,
{
    fn run<'scope>(&'scope self, scope: &Scope<'scope>, work: &Work) {
        if self.quit.load(Ordering::Relaxed) {
            return;
        }
        let _cancel_on_panic = CancelOnPanic(&self.quit);
        let Ok(reader) = fs::read_dir(&work.path) else {
            if (self.visit)(&work.path, None) == WalkState::Quit {
                self.quit.store(true, Ordering::Relaxed);
            }
            return;
        };
        let mut entries = Vec::new();
        let mut complete = true;
        for entry in reader {
            match entry {
                Ok(entry) => entries.push(entry),
                Err(_) => complete = false,
            }
        }
        let listing = if complete {
            Some(entries.as_slice())
        } else {
            None
        };
        match (self.visit)(&work.path, listing) {
            WalkState::Quit => {
                self.quit.store(true, Ordering::Relaxed);
                return;
            }
            WalkState::Skip => return,
            WalkState::Continue => {}
        }
        let mut directories = entries
            .iter()
            .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
            .peekable();
        if directories.peek().is_none() {
            return;
        }
        // Ignore rules only affect children. Pruned and leaf directories need no matcher.
        let ignore = work.ignore.add_child(&work.path, listing);
        let mut matches = Vec::new();
        for entry in directories {
            if self.quit.load(Ordering::Relaxed) {
                break;
            }
            let path = entry.path();
            let matched = ignore.matched(&path, &mut matches);
            if matched.is_ignore() || (matched.is_none() && is_hidden_entry(entry, &path)) {
                continue;
            }
            let work = Work {
                path,
                ignore: ignore.clone(),
            };
            scope.spawn(move |scope| self.run(scope, &work));
        }
    }
}

/// Rayon propagates panics after draining the scope. Cancel queued callbacks first.
struct CancelOnPanic<'a>(&'a AtomicBool);

impl Drop for CancelOnPanic<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.0.store(true, Ordering::Relaxed);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::Path,
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };

    use super::{WalkState, walk};

    fn collect(root: &Path, custom_ignore_filename: &str) -> Vec<String> {
        let visited = Mutex::new(Vec::new());
        walk(root, custom_ignore_filename, |path, _| {
            visited.lock().unwrap().push(
                path.strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
            WalkState::Continue
        });
        let mut visited = visited.into_inner().unwrap();
        visited.sort();
        visited
    }

    /// Directory callbacks see hidden and ignored entries, while Skip prunes descendants.
    #[test]
    fn raw_entries_and_pruning() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join(".ignore"), "ignored/\n").unwrap();
        fs::write(root.join("prek.toml"), "").unwrap();
        for name in [".hidden", "ignored", "kept", "pruned/child"] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        let visited = Mutex::new(Vec::new());
        walk(root, ".prekignore", |path, entries| {
            let relative = path.strip_prefix(root).unwrap();
            visited.lock().unwrap().push(relative.to_path_buf());
            if relative.as_os_str().is_empty() {
                let mut names: Vec<_> = entries
                    .unwrap()
                    .iter()
                    .map(fs::DirEntry::file_name)
                    .collect();
                names.sort();
                assert_eq!(
                    names,
                    [
                        ".hidden",
                        ".ignore",
                        "ignored",
                        "kept",
                        "prek.toml",
                        "pruned"
                    ]
                );
            } else if relative == Path::new("pruned") {
                return WalkState::Skip;
            } else {
                assert!(entries.unwrap().is_empty());
            }
            WalkState::Continue
        });
        let mut visited = visited.into_inner().unwrap();
        visited.sort();
        assert_eq!(
            visited,
            ["", "kept", "pruned"].map(std::path::PathBuf::from)
        );
    }

    /// Custom rules override .ignore and Git rules, including hidden-directory exclusions.
    #[test]
    fn ignore_precedence() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        for name in [
            ".git/info",
            "by-git",
            "by-ignore",
            "by-exclude",
            "custom-visible",
            "resurrected",
            ".visible",
            "nested/ancestor-blocked",
        ] {
            fs::create_dir_all(root.join(name)).unwrap();
        }
        fs::write(
            root.join(".gitignore"),
            "/by-git/\n/resurrected/\n/.visible/\n/nested/ancestor-blocked/\n",
        )
        .unwrap();
        fs::write(
            root.join(".ignore"),
            "/by-ignore/\n!/resurrected/\n/custom-visible/\n",
        )
        .unwrap();
        fs::write(root.join(".prekignore"), "!/custom-visible/\n!/.visible/\n").unwrap();
        fs::write(root.join(".git/info/exclude"), "/by-exclude/\n").unwrap();
        fs::write(root.join("nested/.gitignore"), "!/ancestor-blocked/\n").unwrap();
        assert_eq!(
            collect(root, ".prekignore"),
            [
                "",
                ".visible",
                "custom-visible",
                "nested",
                "nested/ancestor-blocked",
                "resurrected"
            ]
        );
    }

    /// Caller-selected ignore names apply above and inside the root, even when also a standard name.
    #[test]
    fn custom_ignore_filename() {
        for filename in [".projectignore", ".gitignore"] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path().join("project");
            for name in ["from-parent", "from-child", "kept"] {
                fs::create_dir_all(root.join(name)).unwrap();
            }
            fs::write(temp.path().join(filename), "from-parent/\n").unwrap();
            fs::write(root.join(filename), "from-child/\n").unwrap();
            fs::write(root.join(".prekignore"), "kept/\n").unwrap();
            assert_eq!(collect(&root, filename), ["", "kept"]);
        }
    }

    /// Git rules require a Git or Jujutsu marker; plain directories still honor .ignore.
    #[test]
    fn repository_markers() {
        for marker in [None, Some(".git"), Some(".jj")] {
            let temp = tempfile::tempdir().unwrap();
            let root = temp.path();
            for name in ["by-git", "by-ignore", "kept"] {
                fs::create_dir(root.join(name)).unwrap();
            }
            fs::write(root.join(".gitignore"), "/by-git/\n").unwrap();
            fs::write(root.join(".ignore"), "/by-ignore/\n").unwrap();
            if let Some(marker) = marker {
                fs::create_dir(root.join(marker)).unwrap();
                assert_eq!(collect(root, ".prekignore"), ["", "kept"]);
            } else {
                assert_eq!(collect(root, ".prekignore"), ["", "by-git", "kept"]);
            }
        }
    }

    /// Parent rules apply to a subtree, while nested repositories stop ancestor Git rules.
    #[test]
    fn parent_rules_and_repository_boundaries() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path();
        let root = parent.join("project");
        for name in [
            ".git",
            "project/git-hidden",
            "project/from-ignore",
            "project/nested/.git",
            "project/nested/from-parent",
            "project/open",
        ] {
            fs::create_dir_all(parent.join(name)).unwrap();
        }
        fs::write(
            parent.join(".gitignore"),
            "/project/git-hidden/\n/project/nested/from-parent/\n",
        )
        .unwrap();
        fs::write(parent.join(".ignore"), "/project/from-ignore/\n").unwrap();
        assert_eq!(
            collect(&root, ".prekignore"),
            ["", "nested", "nested/from-parent", "open"]
        );
    }

    /// Empty directories preserve parent rules, and an empty repository resets only Git rules.
    #[test]
    fn rules_across_empty_directories() {
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path();
        for name in [
            ".git",
            "project/a/b/blocked",
            "project/a/b/kept",
            "project/a/b/nested/.git",
            "project/a/b/nested/blocked",
            "project/a/b/nested/custom-blocked",
        ] {
            fs::create_dir_all(parent.join(name)).unwrap();
        }
        fs::write(parent.join(".gitignore"), "blocked/\n").unwrap();
        fs::write(parent.join(".prekignore"), "custom-blocked/\n").unwrap();
        assert_eq!(
            collect(&parent.join("project"), ".prekignore"),
            [
                "",
                "a",
                "a/b",
                "a/b/kept",
                "a/b/nested",
                "a/b/nested/blocked"
            ]
        );
    }

    /// Dot components in absolute and relative roots preserve canonical parent-rule matching.
    #[test]
    fn dot_components_in_root() {
        let temp = tempfile::tempdir_in(".").unwrap();
        fs::create_dir_all(temp.path().join("project/a/blocked")).unwrap();
        fs::create_dir_all(temp.path().join("project/a/kept")).unwrap();
        fs::write(temp.path().join(".ignore"), "/project/a/blocked/\n").unwrap();
        for root in [
            temp.path().join("./project"),
            Path::new("./.")
                .join(temp.path().file_name().unwrap())
                .join("project"),
        ] {
            assert_eq!(collect(&root, ".prekignore"), ["", "a", "a/kept"]);
        }
    }

    /// Git worktree files resolve excludes from the common repository directory.
    #[test]
    fn linked_worktree_excludes() {
        let temp = tempfile::tempdir().unwrap();
        let common = temp.path().join("common");
        let private = common.join("worktrees/checkout");
        let root = temp.path().join("checkout");
        fs::create_dir_all(&private).unwrap();
        fs::create_dir_all(common.join("info")).unwrap();
        fs::create_dir_all(root.join("excluded")).unwrap();
        fs::create_dir_all(root.join("kept")).unwrap();
        fs::write(
            root.join(".git"),
            format!("gitdir: {}\n", private.display()),
        )
        .unwrap();
        fs::write(private.join("commondir"), "../..\n").unwrap();
        fs::write(common.join("info/exclude"), "/excluded/\n").unwrap();
        assert_eq!(collect(&root, ".prekignore"), ["", "kept"]);
    }

    /// BOM-prefixed ignore files still apply valid rules when another glob is invalid.
    #[test]
    fn partially_invalid_ignore_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("ignored")).unwrap();
        fs::create_dir(root.join("kept")).unwrap();
        fs::write(root.join(".ignore"), "\u{feff}ignored/\n*[z-a]\n").unwrap();
        assert_eq!(collect(root, ".prekignore"), ["", "kept"]);
    }

    /// A symlinked root keeps canonical parent rules; child symlinks are not traversed.
    #[cfg(unix)]
    #[test]
    fn symlinked_root_and_children() {
        use std::os::unix::fs::symlink;
        let temp = tempfile::tempdir().unwrap();
        let parent = temp.path().join("parent");
        let root = parent.join("project");
        fs::create_dir_all(root.join("ignored")).unwrap();
        fs::create_dir_all(root.join("kept")).unwrap();
        fs::write(parent.join(".ignore"), "/project/ignored/\n").unwrap();
        symlink(root.join("kept"), root.join("link")).unwrap();
        symlink("missing", root.join("broken")).unwrap();
        let alias = temp.path().join("alias");
        symlink(&root, &alias).unwrap();
        assert_eq!(collect(&alias, ".prekignore"), ["", "kept"]);
    }

    /// An unreadable directory gets None, so callers can still probe known config names.
    #[cfg(unix)]
    #[test]
    fn unreadable_listing() {
        use std::os::unix::fs::PermissionsExt;
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::write(root.join("prek.toml"), "").unwrap();
        let permissions = fs::metadata(root).unwrap().permissions();
        fs::set_permissions(root, fs::Permissions::from_mode(0o111)).unwrap();
        if fs::read_dir(root).is_ok() {
            // Privileged users may still be able to enumerate the directory.
            fs::set_permissions(root, permissions).unwrap();
            return;
        }
        let visited = AtomicUsize::new(0);
        walk(root, ".prekignore", |path, entries| {
            assert_eq!(path, root);
            assert!(entries.is_none());
            assert!(path.join("prek.toml").is_file());
            visited.fetch_add(1, Ordering::Relaxed);
            WalkState::Continue
        });
        fs::set_permissions(root, permissions).unwrap();
        assert_eq!(visited.into_inner(), 1);
    }

    /// Quitting at the root terminates idle workers without scheduling child callbacks.
    #[test]
    fn quit_before_children() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir(temp.path().join("child")).unwrap();
        let visited = AtomicUsize::new(0);
        walk(temp.path(), ".prekignore", |_, _| {
            visited.fetch_add(1, Ordering::Relaxed);
            WalkState::Quit
        });
        assert_eq!(visited.into_inner(), 1);
    }

    /// A callback panic cancels waiting workers and propagates to the caller.
    #[test]
    fn panic_cancels_workers() {
        let temp = tempfile::tempdir().unwrap();
        let visited = AtomicUsize::new(0);
        let result = std::panic::catch_unwind(|| {
            walk(temp.path(), ".prekignore", |_, _| {
                visited.fetch_add(1, Ordering::Relaxed);
                panic!("callback panic");
            });
        });
        assert!(result.is_err());
        assert_eq!(visited.into_inner(), 1);
    }
}
