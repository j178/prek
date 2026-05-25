use std::io::Write as _;
use std::path::Path;
use std::str;

use anyhow::{Context, Result};
use rustc_hash::FxHashSet;

use crate::git;
use crate::hook::Hook;

const ORDINARY_CHANGED_ENTRY_MARKER: &str = "1";
const PERMS_LINK: u32 = 0o120_000;
const PERMS_NONEXIST: u32 = 0;

pub(crate) async fn destroyed_symlinks(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let status_output = git_status_output(hook.work_dir()).await?;
    let entries = status_output
        .split(|&byte| byte == b'\0')
        .filter_map(|entry| match parse_ordinary_changed_entry(entry) {
            Ok(Some(entry)) => Some(Ok(entry)),
            Ok(None) => None,
            Err(err) => Some(Err(err)),
        });

    let destroyed_links = find_destroyed_symlinks(hook, filenames, entries).await?;
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

async fn git_status_output(work_dir: &Path) -> Result<Vec<u8>> {
    Ok(git::git_cmd("git status")?
        .current_dir(work_dir)
        .arg("status")
        .arg("--porcelain=v2")
        .arg("-z")
        // Query the whole project with a single pathspec to avoid one-argv-entry-per-file
        // command lines that can exceed the platform limit for very large commits.
        .arg("--")
        .arg(".")
        .check(true)
        .output()
        .await?
        .stdout)
}

async fn find_destroyed_symlinks<'a>(
    hook: &Hook,
    filenames: &[&Path],
    entries: impl IntoIterator<Item = Result<OrdinaryChangedEntry<'a>>>,
) -> Result<Vec<&'a Path>> {
    if filenames.is_empty() {
        return Ok(Vec::new());
    }

    let filenames = filenames.iter().copied().collect::<FxHashSet<_>>();
    let relative_prefix = hook.project().relative_path();
    let mut destroyed_links = Vec::new();

    for entry in entries {
        let entry = entry?;
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

        if is_destroyed_symlink(hook.work_dir(), &entry).await? {
            destroyed_links.push(entry_path);
        }
    }

    Ok(destroyed_links)
}
// Parsed from `git status --porcelain=v2` ordinary changed entries:
// `1 <XY> <sub> <mH> <mI> <mW> <hH> <hI> <path>`
// See: https://git-scm.com/docs/git-status#_changed_tracked_entries
#[derive(Debug, PartialEq, Eq)]
struct OrdinaryChangedEntry<'a> {
    // `<mH>`: The octal file mode in HEAD.
    head_mode: u32,
    // `<mI>`: The octal file mode in the index.
    index_mode: u32,
    // `<hH>`: The object name in HEAD.
    head_hash: &'a str,
    // `<hI>`: The object name in the index.
    index_hash: &'a str,
    // `<path>`: The pathname, reported relative to the repository root when
    // using `git status --porcelain=v2 -z`.
    path: &'a Path,
}

fn parse_ordinary_changed_entry(line: &[u8]) -> Result<Option<OrdinaryChangedEntry<'_>>> {
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
    let head_hash = str::from_utf8(next_field()?)?;
    let index_hash = str::from_utf8(next_field()?)?;
    let path = Path::new(str::from_utf8(next_field()?)?);

    Ok(Some(OrdinaryChangedEntry {
        head_mode,
        index_mode,
        head_hash,
        index_hash,
        path,
    }))
}

async fn is_destroyed_symlink(work_dir: &Path, entry: &OrdinaryChangedEntry<'_>) -> Result<bool> {
    // If the staged blob is byte-for-byte identical to the old symlink blob, we
    // already know this is a destroyed symlink: the path used to be stored as a
    // symlink target and is now staged as a regular file with the same contents.
    if entry.head_hash == entry.index_hash {
        return Ok(true);
    }

    let index_size = git_object_size(work_dir, entry.index_hash).await?;
    let head_size = git_object_size(work_dir, entry.head_hash).await?;
    // Formatting hooks may have appended a trailing newline or converted LF to
    // CRLF, so allow the staged file to grow by at most two bytes before doing
    // the more expensive content comparison.
    if index_size > head_size.saturating_add(2) {
        return Ok(false);
    }

    let head_content = git_object_content(work_dir, entry.head_hash).await?;
    let index_content = git_object_content(work_dir, entry.index_hash).await?;

    // Match upstream behavior by ignoring trailing ASCII whitespace here. That
    // keeps "path", "path\n", and "path\r\n" in the destroyed-symlink bucket.
    Ok(head_content.trim_ascii_end() == index_content.trim_ascii_end())
}

async fn git_object_size(work_dir: &Path, object: &str) -> Result<u64> {
    let output = git::git_cmd("git cat-file")?
        .current_dir(work_dir)
        .arg("cat-file")
        .arg("-s")
        .arg(object)
        .check(true)
        .output()
        .await?;

    Ok(str::from_utf8(&output.stdout)?.trim_ascii().parse()?)
}

async fn git_object_content(work_dir: &Path, object: &str) -> Result<Vec<u8>> {
    Ok(git::git_cmd("git cat-file")?
        .current_dir(work_dir)
        .arg("cat-file")
        .arg("-p")
        .arg(object)
        .check(true)
        .output()
        .await?
        .stdout)
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
