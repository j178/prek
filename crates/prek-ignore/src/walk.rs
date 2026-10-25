use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering as AtomicOrdering},
    },
};

use crossbeam_deque::{Stealer, Worker as Deque};

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
    let threads = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(12);
    let shared = Arc::new(Shared {
        quit_now: AtomicBool::new(false),
        active_workers: AtomicUsize::new(threads),
        wait_lock: Mutex::new(()),
        wakeup: Condvar::new(),
    });
    let initial = Work {
        path: root.to_path_buf(),
        ignore: Ignore::new(root, custom_ignore_filename),
    };
    let stacks = Stack::new_for_each_thread(threads, initial);
    std::thread::scope(|scope| {
        for stack in stacks {
            let worker = Worker {
                stack,
                shared: shared.clone(),
                visit: &visit,
            };
            scope.spawn(move || worker.run());
        }
    });
}

type Visitor<'a> = dyn Fn(&Path, Option<&[fs::DirEntry]>) -> WalkState + Sync + 'a;

#[derive(Debug)]
enum Message {
    Work(Work),
    Quit,
}

#[derive(Debug)]
struct Work {
    path: PathBuf,
    ignore: Ignore,
}

struct Shared {
    quit_now: AtomicBool,
    active_workers: AtomicUsize,
    wait_lock: Mutex<()>,
    wakeup: Condvar,
}

struct Worker<'a> {
    stack: Stack,
    shared: Arc<Shared>,
    visit: &'a Visitor<'a>,
}

impl Worker<'_> {
    fn run(mut self) {
        while let Some(work) = self.get_work() {
            if self.run_one(&work) == WalkState::Quit {
                self.quit_now();
            }
        }
    }

    fn run_one(&self, work: &Work) -> WalkState {
        let Ok(reader) = fs::read_dir(&work.path) else {
            return (self.visit)(&work.path, None);
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
        let ignore = work.ignore.add_child(&work.path, listing);
        let state = (self.visit)(&work.path, listing);
        if state != WalkState::Continue {
            return state;
        }
        for entry in entries {
            // Only directories can contain projects. File entries still remain
            // available in the listing above for configuration discovery.
            if !entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                continue;
            }
            let path = entry.path();
            let matched = ignore.matched(&path);
            if matched.is_ignore() || (matched.is_none() && is_hidden_entry(&entry, &path)) {
                continue;
            }
            self.send(Work {
                path,
                ignore: ignore.clone(),
            });
        }
        WalkState::Continue
    }
}

/// A work-stealing stack.
#[derive(Debug)]
struct Stack {
    /// This thread's index.
    index: usize,
    /// The thread-local stack.
    deque: Deque<Message>,
    /// The work stealers.
    stealers: Arc<[Stealer<Message>]>,
}

impl Stack {
    /// Start at the root; workers steal subdirectories as they are discovered.
    fn new_for_each_thread(threads: usize, root: Work) -> Vec<Stack> {
        // Using new_lifo() ensures each worker operates depth-first, not
        // breadth-first. We do depth-first because a breadth first traversal
        // on wide directories with a lot of gitignores is disastrous (for
        // example, searching a directory tree containing all of crates.io).
        let deques: Vec<Deque<Message>> = std::iter::repeat_with(Deque::new_lifo)
            .take(threads)
            .collect();
        let stealers =
            Arc::<[Stealer<Message>]>::from(deques.iter().map(Deque::stealer).collect::<Vec<_>>());
        let stacks: Vec<Stack> = deques
            .into_iter()
            .enumerate()
            .map(|(index, deque)| Stack {
                index,
                deque,
                stealers: stealers.clone(),
            })
            .collect();
        if let Some(stack) = stacks.first() {
            stack.push(Message::Work(root));
        }
        stacks
    }

    /// Push a message.
    fn push(&self, msg: Message) {
        self.deque.push(msg);
    }

    /// Pop a message.
    fn pop(&self) -> Option<Message> {
        self.deque.pop().or_else(|| self.steal())
    }

    /// Steal a message from another queue.
    fn steal(&self) -> Option<Message> {
        // For fairness, try to steal from index + 1, index + 2, ... len - 1,
        // then wrap around to 0, 1, ... index - 1.
        let (left, right) = self.stealers.split_at(self.index);
        // Don't steal from ourselves
        let right = &right[1..];

        right
            .iter()
            .chain(left.iter())
            .map(|s| s.steal_batch_and_pop(&self.deque))
            .find_map(crossbeam_deque::Steal::success)
    }
}

impl Worker<'_> {
    /// Returns the next directory to descend into.
    ///
    /// If all work has been exhausted, then this returns None. The worker
    /// should then subsequently quit.
    fn get_work(&mut self) -> Option<Work> {
        let mut value = self.recv();
        loop {
            // Simulate a priority channel: If quit_now flag is set, we can
            // receive only quit messages.
            if self.is_quit_now() {
                value = Some(Message::Quit);
            }
            match value {
                Some(Message::Work(work)) => {
                    return Some(work);
                }
                Some(Message::Quit) => {
                    // Repeat quit message to wake up sleeping threads, if
                    // any. The domino effect will ensure that every thread
                    // will quit.
                    self.send_quit();
                    return None;
                }
                None => {
                    if self.deactivate_worker() == 0 {
                        // If deactivate_worker() returns 0, every worker thread
                        // is currently within the critical section between the
                        // acquire in deactivate_worker() and the release in
                        // activate_worker() below.  For this to happen, every
                        // worker's local deque must be simultaneously empty,
                        // meaning there is no more work left at all.
                        self.send_quit();
                        return None;
                    }
                    // Wait for next `Work` or `Quit` message.
                    let mut guard = self
                        .shared
                        .wait_lock
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner);
                    loop {
                        if self.is_quit_now() {
                            return None;
                        }
                        if let Some(v) = self.recv() {
                            self.activate_worker();
                            value = Some(v);
                            break;
                        }
                        // Poll for new work without synchronizing every push.
                        // Termination wakes all waiters immediately instead of
                        // waiting for this timeout on each worker.
                        let dur = std::time::Duration::from_millis(1);
                        guard = self
                            .shared
                            .wakeup
                            .wait_timeout(guard, dur)
                            .unwrap_or_else(std::sync::PoisonError::into_inner)
                            .0;
                    }
                }
            }
        }
    }

    /// Indicates that all workers should quit immediately.
    fn quit_now(&self) {
        self.shared.quit_now.store(true, AtomicOrdering::SeqCst);
        self.wake_workers();
    }

    /// Returns true if this worker should quit immediately.
    fn is_quit_now(&self) -> bool {
        self.shared.quit_now.load(AtomicOrdering::SeqCst)
    }

    /// Send work.
    fn send(&self, work: Work) {
        self.stack.push(Message::Work(work));
    }

    /// Send a quit message.
    fn send_quit(&self) {
        self.stack.push(Message::Quit);
        self.wake_workers();
    }

    /// Wake idle workers promptly when the walk finishes or is cancelled.
    fn wake_workers(&self) {
        // Pair with the queue check under this lock so termination cannot
        // arrive between checking the queue and going to sleep. The lock
        // protects no data, so poisoning does not prevent shutdown.
        let _guard = self
            .shared
            .wait_lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.shared.wakeup.notify_all();
    }

    /// Receive work.
    fn recv(&self) -> Option<Message> {
        self.stack.pop()
    }

    /// Deactivates a worker and returns the number of currently active workers.
    fn deactivate_worker(&self) -> usize {
        self.shared
            .active_workers
            .fetch_sub(1, AtomicOrdering::Acquire)
            - 1
    }

    /// Reactivates a worker.
    fn activate_worker(&self) {
        self.shared
            .active_workers
            .fetch_add(1, AtomicOrdering::Release);
    }
}

impl Drop for Worker<'_> {
    fn drop(&mut self) {
        if std::thread::panicking() {
            self.quit_now();
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

    fn collect(root: &Path) -> Vec<String> {
        let visited = Mutex::new(Vec::new());
        walk(root, ".prekignore", |path, _| {
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
            collect(root),
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
                assert_eq!(collect(root), ["", "kept"]);
            } else {
                assert_eq!(collect(root), ["", "by-git", "kept"]);
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
        assert_eq!(collect(&root), ["", "nested", "nested/from-parent", "open"]);
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
        assert_eq!(collect(&root), ["", "kept"]);
    }

    /// BOM-prefixed ignore files still apply valid rules when another glob is invalid.
    #[test]
    fn partially_invalid_ignore_file() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path();
        fs::create_dir(root.join("ignored")).unwrap();
        fs::create_dir(root.join("kept")).unwrap();
        fs::write(root.join(".ignore"), "\u{feff}ignored/\n*[z-a]\n").unwrap();
        assert_eq!(collect(root), ["", "kept"]);
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
        assert_eq!(collect(&alias), ["", "kept"]);
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
