use std::path::{Path, PathBuf};

use anyhow::Result;
use rustc_hash::FxHashSet;

use crate::git::get_added_files;
use crate::hook::Hook;

pub(crate) async fn check_case_conflict(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let work_dir = hook.work_dir();

    // Get all files in the repo
    let output = tokio::process::Command::new("git")
        .arg("ls-files")
        .current_dir(work_dir)
        .output()
        .await?;

    let repo_files: FxHashSet<PathBuf> = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(PathBuf::from)
        .collect();

    // Add directories for repo files
    let mut repo_files_with_dirs = repo_files.clone();
    for file in &repo_files {
        repo_files_with_dirs.extend(get_parent_dirs(file));
    }

    // Get relevant files (filenames + added files)
    let added = get_added_files(work_dir).await?;
    let mut relevant_files: FxHashSet<PathBuf> =
        filenames.iter().map(|p| p.to_path_buf()).collect();
    relevant_files.extend(added);

    // Add directories for relevant files
    let mut relevant_files_with_dirs = relevant_files.clone();
    for file in &relevant_files {
        relevant_files_with_dirs.extend(get_parent_dirs(file));
    }

    // Remove relevant files from repo files (we only check conflicts with existing files)
    for file in &relevant_files_with_dirs {
        repo_files_with_dirs.remove(file);
    }

    let mut retv = 0;
    let mut conflicts = FxHashSet::default();

    // Check for conflicts between new files and existing files
    let repo_lower = to_lowercase_set(&repo_files_with_dirs);
    let relevant_lower = to_lowercase_set(&relevant_files_with_dirs);
    conflicts.extend(repo_lower.intersection(&relevant_lower).cloned());

    // Check for conflicts among new files themselves
    let mut lowercase_relevant = to_lowercase_set(&relevant_files_with_dirs);
    for filename in &relevant_files_with_dirs {
        let lower = filename.to_string_lossy().to_lowercase();
        if lowercase_relevant.contains(&lower) {
            lowercase_relevant.remove(&lower);
        } else {
            conflicts.insert(lower);
        }
    }

    let mut output = Vec::new();
    if !conflicts.is_empty() {
        let mut conflicting_files: Vec<PathBuf> = repo_files_with_dirs
            .union(&relevant_files_with_dirs)
            .filter(|f| conflicts.contains(&f.to_string_lossy().to_lowercase()))
            .cloned()
            .collect();
        conflicting_files.sort();

        for filename in conflicting_files {
            let line = format!(
                "Case-insensitivity conflict found: {}\n",
                filename.display()
            );
            output.extend(line.into_bytes());
        }
        retv = 1;
    }

    Ok((retv, output))
}

fn get_parent_dirs(file: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = file;
    while let Some(parent) = current.parent() {
        if parent == Path::new("") {
            break;
        }
        dirs.push(parent.to_path_buf());
        current = parent;
    }
    dirs
}

fn to_lowercase_set(files: &FxHashSet<PathBuf>) -> FxHashSet<String> {
    files
        .iter()
        .map(|p| p.to_string_lossy().to_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_parent_dirs() {
        let parents = get_parent_dirs(Path::new("foo/bar/baz.txt"));
        assert_eq!(parents, vec![Path::new("foo/bar"), Path::new("foo")]);

        let parents = get_parent_dirs(Path::new("single.txt"));
        assert!(parents.is_empty());

        let parents = get_parent_dirs(Path::new("a/b/c/d.txt"));
        assert_eq!(
            parents,
            vec![Path::new("a/b/c"), Path::new("a/b"), Path::new("a")]
        );
    }

    #[test]
    fn test_to_lowercase_set() {
        let mut files = FxHashSet::default();
        files.insert(PathBuf::from("Foo.txt"));
        files.insert(PathBuf::from("BAR.txt"));
        files.insert(PathBuf::from("baz.TXT"));

        let lower = to_lowercase_set(&files);
        assert!(lower.contains("foo.txt"));
        assert!(lower.contains("bar.txt"));
        assert!(lower.contains("baz.txt"));
        assert_eq!(lower.len(), 3);
    }

    #[test]
    fn test_get_parent_dirs_nested() {
        let parents = get_parent_dirs(Path::new("a/b/c/d/e/f.txt"));
        assert_eq!(
            parents,
            vec![
                Path::new("a/b/c/d/e"),
                Path::new("a/b/c/d"),
                Path::new("a/b/c"),
                Path::new("a/b"),
                Path::new("a")
            ]
        );
    }

    #[test]
    fn test_get_parent_dirs_no_slash() {
        let parents = get_parent_dirs(Path::new("file.txt"));
        assert!(parents.is_empty());
    }

    #[test]
    fn test_to_lowercase_set_empty() {
        let files: FxHashSet<PathBuf> = FxHashSet::default();
        let lower = to_lowercase_set(&files);
        assert!(lower.is_empty());
    }

    #[test]
    fn test_to_lowercase_set_mixed_case() {
        let mut files = FxHashSet::default();
        files.insert(PathBuf::from("FooBar.TXT"));
        files.insert(PathBuf::from("FOOBAR.txt"));
        files.insert(PathBuf::from("foobar.Txt"));

        let lower = to_lowercase_set(&files);
        // All three should map to the same lowercase version
        assert!(lower.contains("foobar.txt"));
        assert_eq!(lower.len(), 1);
    }
}
