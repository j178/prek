use std::collections::hash_map::Entry;
use std::path::Path;

use anyhow::Result;
use rustc_hash::FxHashMap;
use rustc_hash::FxHashSet;

use crate::git;
use crate::hook::Hook;

pub(crate) async fn check_case_conflict(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let work_dir = hook.work_dir();

    // Get all files in the repo.
    let repo_files = git::ls_files(work_dir, Path::new(".")).await?;
    let mut repo_files_with_dirs: FxHashSet<&Path> = FxHashSet::default();
    for path in &repo_files {
        insert_path_and_parents(&mut repo_files_with_dirs, path);
    }

    // Get relevant files (filenames + added files) and include their parent directories.
    let added = git::get_added_files(work_dir).await?;
    let mut relevant_files_with_dirs: FxHashSet<&Path> = FxHashSet::default();
    for filename in filenames {
        insert_path_and_parents(&mut relevant_files_with_dirs, filename);
    }
    for path in &added {
        insert_path_and_parents(&mut relevant_files_with_dirs, path);
    }

    // Remove relevant files from repo files (avoid self-conflicts).
    for file in &relevant_files_with_dirs {
        repo_files_with_dirs.remove(file);
    }

    // Compute conflicts:
    // 1) relevant vs repo (case-insensitive intersection)
    // 2) relevant vs relevant (case-insensitive duplicates)
    let mut repo_lower: FxHashSet<String> = FxHashSet::default();
    repo_lower.reserve(repo_files_with_dirs.len());
    for path in &repo_files_with_dirs {
        repo_lower.insert(lower_key(path));
    }

    let mut conflicts: FxHashSet<String> = FxHashSet::default();
    let mut relevant_lower_counts: FxHashMap<String, u8> = FxHashMap::default();
    relevant_lower_counts.reserve(relevant_files_with_dirs.len());

    for path in &relevant_files_with_dirs {
        let lower = lower_key(path);

        if repo_lower.contains(&lower) {
            conflicts.insert(lower.clone());
        }

        match relevant_lower_counts.entry(lower) {
            Entry::Vacant(entry) => {
                entry.insert(1);
            }
            Entry::Occupied(mut entry) => {
                let count = entry.get_mut();
                *count = count.saturating_add(1);
                if *count == 2 {
                    // Only mark the conflict on the *first* duplicate to avoid repeated
                    // cloning/inserting for the 3rd+ occurrences of the same lowercase key.
                    conflicts.insert(entry.key().clone());
                }
            }
        }
    }

    let mut output = Vec::new();
    if conflicts.is_empty() {
        return Ok((0, output));
    }

    // The sets are disjoint at this point (relevant removed from repo), so we can just chain.
    let mut conflicting_files: Vec<_> = repo_files_with_dirs
        .iter()
        .chain(relevant_files_with_dirs.iter())
        .filter(|path| conflicts.contains(&lower_key(path)))
        .collect();
    conflicting_files.sort();

    for filename in conflicting_files {
        let line = format!(
            "Case-insensitivity conflict found: {}\n",
            filename.display()
        );
        output.extend(line.into_bytes());
    }

    Ok((1, output))
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
