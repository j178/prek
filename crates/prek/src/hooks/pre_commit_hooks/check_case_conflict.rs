use std::io::Write;
use std::path::Path;

use anyhow::Result;
use rustc_hash::FxHashSet;

use crate::git;
use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{FilenamesArgs, hook_filenames, parse_hook_args};

/// Runs the `check-case-conflict` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: FilenamesArgs = parse_hook_args(hook)?;
    let filenames = hook_filenames(&args.filenames, filenames).collect::<Vec<_>>();
    let work_dir = hook.work_dir();

    let repo_files = git::ls_files(work_dir, [Path::new(".")]).await?;
    let mut all_files = FxHashSet::default();
    for path in &repo_files {
        insert_path_and_parents(&mut all_files, path);
    }
    for filename in &filenames {
        insert_path_and_parents(&mut all_files, filename);
    }

    let mut seen = FxHashSet::default();
    let mut conflicts = FxHashSet::default();
    for path in &all_files {
        if !seen.insert(lower_key(path)) {
            conflicts.insert(lower_key(path));
        }
    }

    let mut output = Vec::new();
    if conflicts.is_empty() {
        return Ok(HookOutput::unchanged(0, output));
    }

    // Newly staged files are already in the index. Query them only when a
    // conflict exists, to distinguish relevant conflicts from existing ones.
    let added = git::staged_added_files(work_dir).await?;
    let mut relevant_files = FxHashSet::default();
    for filename in &filenames {
        insert_path_and_parents(&mut relevant_files, filename);
    }
    for path in &added {
        insert_path_and_parents(&mut relevant_files, path);
    }
    let relevant_conflicts: FxHashSet<_> = relevant_files
        .iter()
        .map(|path| lower_key(path))
        .filter(|lower| conflicts.contains(lower))
        .collect();
    if relevant_conflicts.is_empty() {
        return Ok(HookOutput::unchanged(0, output));
    }

    let mut conflicting_files: Vec<_> = all_files
        .iter()
        .filter(|path| relevant_conflicts.contains(&lower_key(path)))
        .collect();
    conflicting_files.sort();

    for filename in conflicting_files {
        writeln!(
            output,
            "Case-insensitivity conflict found: {}",
            filename.display()
        )?;
    }

    Ok(HookOutput::unchanged(1, output))
}

fn insert_path_and_parents<'p>(set: &mut FxHashSet<&'p Path>, file: &'p Path) {
    set.insert(file);

    let mut current = file;
    while let Some(parent) = current.parent() {
        if parent.as_os_str().is_empty() {
            break;
        }
        set.insert(parent);
        current = parent;
    }
}

fn lower_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_insert_path_and_parents() {
        let mut set: FxHashSet<&Path> = FxHashSet::default();
        insert_path_and_parents(&mut set, Path::new("foo/bar/baz.txt"));
        assert!(set.contains(Path::new("foo/bar/baz.txt")));
        assert!(set.contains(Path::new("foo/bar")));
        assert!(set.contains(Path::new("foo")));
        assert_eq!(set.len(), 3);

        let mut set: FxHashSet<&Path> = FxHashSet::default();
        insert_path_and_parents(&mut set, Path::new("single.txt"));
        assert!(set.contains(Path::new("single.txt")));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_insert_path_and_parents_nested() {
        let mut set: FxHashSet<&Path> = FxHashSet::default();
        insert_path_and_parents(&mut set, Path::new("a/b/c/d/e/f.txt"));
        for expected in [
            "a/b/c/d/e/f.txt",
            "a/b/c/d/e",
            "a/b/c/d",
            "a/b/c",
            "a/b",
            "a",
        ] {
            assert!(set.contains(Path::new(expected)));
        }
    }

    #[test]
    fn test_insert_path_and_parents_no_slash() {
        let mut set: FxHashSet<&Path> = FxHashSet::default();
        insert_path_and_parents(&mut set, Path::new("file.txt"));
        assert_eq!(set.len(), 1);
    }

    #[test]
    fn test_lower_key() {
        assert_eq!(lower_key(Path::new("Foo.txt")), "foo.txt");
        assert_eq!(lower_key(Path::new("BAR.txt")), "bar.txt");
        assert_eq!(lower_key(Path::new("baz.TXT")), "baz.txt");
    }
}
