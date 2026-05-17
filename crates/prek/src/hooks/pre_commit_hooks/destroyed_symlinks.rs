use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::str;

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;

use crate::git;
use crate::hook::Hook;

const ORDINARY_CHANGED_ENTRY_MARKER: &str = "1";
const PERMS_LINK: u32 = 0o120_000;
const PERMS_NONEXIST: u32 = 0;

pub(crate) async fn destroyed_symlinks(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let entries = git::staged_changed_entries(hook.work_dir())?;
    let destroyed_links = find_destroyed_symlinks(hook, filenames, entries)?;
    if destroyed_links.is_empty() {
        return Ok((0, Vec::new()));
    }

    let mut output = Vec::new();
    writeln!(output, "Destroyed symlinks:")?;
    for destroyed_link in &destroyed_links {
        writeln!(output, "- {}", destroyed_link.display())?;
    }
    let destroyed_links_shell = destroyed_links
        .iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    writeln!(output, "You should unstage affected files:")?;
    writeln!(
        output,
        "\tgit reset HEAD -- {}",
        shlex::try_join(destroyed_links_shell.iter().map(String::as_str))?
    )?;
    writeln!(
        output,
        "And retry commit. As a long term solution you may try to explicitly tell git that your environment does not support symlinks:"
    )?;
    writeln!(output, "\tgit config core.symlinks false")?;

    Ok((1, output))
}

fn find_destroyed_symlinks(
    hook: &Hook,
    filenames: &[&Path],
    entries: impl IntoIterator<Item = git::StagedChangedEntry>,
) -> Result<Vec<PathBuf>> {
    if filenames.is_empty() {
        return Ok(Vec::new());
    }

    let filenames = filenames.iter().copied().collect::<FxHashSet<_>>();
    let relative_prefix = hook.project().relative_path();
    let mut destroyed_links = Vec::new();

    for entry in entries {
        // `git status -z` reports paths relative to the repository root, so strip the project
        // prefix before comparing against the requested filenames.
        let Ok(entry_path) = entry.path.strip_prefix(relative_prefix) else {
            continue;
        };
        if !filenames.contains(entry_path) {
            continue;
        }

        // We only care about entries that used to be symlinks in HEAD but are
        // now staged as regular files. Still-a-symlink entries are fine, and a
        // deleted symlink is not a "destroyed symlink" case.
        if entry.head_mode != PERMS_LINK
            || entry.index_mode == PERMS_LINK
            || entry.index_mode == PERMS_NONEXIST
        {
            continue;
        }

        if is_destroyed_symlink(hook.work_dir(), &entry)? {
            destroyed_links.push(entry_path.to_path_buf());
        }
    }

    Ok(destroyed_links)
}
// Parsed from `git status --porcelain=v2` ordinary changed entries:
// `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
// See: https://git-scm.com/docs/git-status#_changed_tracked_entries
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct OrdinaryChangedEntry {
    // `<mH>`: The octal file mode in HEAD.
    pub(crate) head_mode: u32,
    // `<mI>`: The octal file mode in the index.
    pub(crate) index_mode: u32,
    // `<hH>`: The object name in HEAD.
    pub(crate) head_hash: String,
    // `<hI>`: The object name in the index.
    pub(crate) index_hash: String,
    // `<path>`: The pathname, reported relative to the repository root when
    // using `git status --porcelain=v2 -z`.
    pub(crate) path: PathBuf,
}

fn parse_ordinary_changed_entry(line: &[u8]) -> Result<Option<OrdinaryChangedEntry>> {
    if line.is_empty() {
        return Ok(None);
    }

    let mut fields = line.splitn(9, |&byte| byte == b' ');
    let mut next_field = || {
        fields
            .next()
            .context("malformed `git status --porcelain=v2` output")
    };
    let parse_mode = |field| -> Result<u32> { Ok(u32::from_str_radix(str::from_utf8(field)?, 8)?) };
    let marker = next_field()?;
    // `git status --porcelain=v2` emits several record types. We only parse
    // ordinary changed entries (`1 ...`) here and let callers skip the rest.
    if marker != ORDINARY_CHANGED_ENTRY_MARKER.as_bytes() {
        return Ok(None);
    }

    let _xy = next_field()?;
    let _sub = next_field()?;
    let head_mode = parse_mode(next_field()?)?;
    let index_mode = parse_mode(next_field()?)?;
    let _mode_worktree = next_field()?;
    let head_hash = str::from_utf8(next_field()?)?.to_string();
    let index_hash = str::from_utf8(next_field()?)?.to_string();
    let path = PathBuf::from(str::from_utf8(next_field()?)?);

    Ok(Some(OrdinaryChangedEntry {
        head_mode,
        index_mode,
        head_hash,
        index_hash,
        path,
    }))
}

fn is_destroyed_symlink(work_dir: &Path, entry: &git::StagedChangedEntry) -> Result<bool> {
    // If the staged blob is byte-for-byte identical to the old symlink blob, we
    // already know this is a destroyed symlink: the path used to be stored as a
    // symlink target and is now staged as a regular file with the same contents.
    if entry.head_hash == entry.index_hash {
        return Ok(true);
    }

    let index_size = git_object_size(work_dir, &entry.index_hash)?;
    let head_size = git_object_size(work_dir, &entry.head_hash)?;
    // Formatting hooks may have appended a trailing newline or converted LF to
    // CRLF, so allow the staged file to grow by at most two bytes before doing
    // the more expensive content comparison.
    if index_size > head_size.saturating_add(2) {
        return Ok(false);
    }

    let head_content = git_object_content(work_dir, &entry.head_hash)?;
    let index_content = git_object_content(work_dir, &entry.index_hash)?;

    // Match upstream behavior by ignoring trailing ASCII whitespace here. That
    // keeps "path", "path\n", and "path\r\n" in the destroyed-symlink bucket.
    Ok(head_content.trim_ascii_end() == index_content.trim_ascii_end())
}

fn git_object_size(work_dir: &Path, object: &str) -> Result<u64> {
    Ok(git::object_size(work_dir, object)?)
}

fn git_object_content(work_dir: &Path, object: &str) -> Result<Vec<u8>> {
    Ok(git::object_content(work_dir, object)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_ordinary_changed_entry_supports_spaces_in_paths() -> Result<()> {
        let entry = parse_ordinary_changed_entry(
            b"1 M. N... 120000 100644 100644 headhash indexhash path with spaces.txt",
        )?
        .expect("entry should parse");

        assert_eq!(entry.head_mode, PERMS_LINK);
        assert_eq!(entry.index_mode, 0o100_644);
        assert_eq!(entry.head_hash, "headhash");
        assert_eq!(entry.index_hash, "indexhash");
        assert_eq!(entry.path, Path::new("path with spaces.txt"));

        Ok(())
    }
}
