use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::str;

use anyhow::{Context, Result};
use rustc_hash::FxHashMap;

use crate::git;
use crate::hook::Hook;

const ORDINARY_CHANGED_ENTRY_MARKER: &str = "1";
const PERMS_LINK: &str = "120000";
const PERMS_NONEXIST: &str = "000000";

pub(crate) async fn destroyed_symlinks(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let destroyed_links = find_destroyed_symlinks(hook, filenames).await?;
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

async fn find_destroyed_symlinks(hook: &Hook, filenames: &[&Path]) -> Result<Vec<PathBuf>> {
    if filenames.is_empty() {
        return Ok(Vec::new());
    }

    let relative_prefix = hook.project().relative_path();
    let display_paths = filenames
        .iter()
        .map(|filename| {
            (
                normalize_filename(relative_prefix, filename).to_path_buf(),
                (*filename).to_path_buf(),
            )
        })
        .collect::<FxHashMap<_, _>>();

    let output = git::git_cmd("git status")?
        .current_dir(hook.work_dir())
        .arg("status")
        .arg("--porcelain=v2")
        .arg("-z")
        .arg("--")
        .args(display_paths.keys())
        .check(true)
        .output()
        .await?;

    let mut destroyed_links = Vec::new();
    for entry in output.stdout.split(|&byte| byte == b'\0') {
        let Some(entry) = parse_ordinary_changed_entry(entry)? else {
            continue;
        };

        if entry.mode_head != PERMS_LINK
            || matches!(entry.mode_index.as_str(), PERMS_LINK | PERMS_NONEXIST)
        {
            continue;
        }

        if is_destroyed_symlink(hook.work_dir(), &entry).await? {
            let display_path = display_paths
                .get(&entry.path)
                .cloned()
                .unwrap_or_else(|| relative_prefix.join(&entry.path));
            destroyed_links.push(display_path);
        }
    }

    Ok(destroyed_links)
}

fn normalize_filename<'a>(relative_prefix: &Path, filename: &'a Path) -> &'a Path {
    if relative_prefix.as_os_str().is_empty() {
        filename
    } else {
        filename.strip_prefix(relative_prefix).unwrap_or(filename)
    }
}

#[derive(Debug, PartialEq, Eq)]
struct OrdinaryChangedEntry {
    mode_head: String,
    mode_index: String,
    hash_head: String,
    hash_index: String,
    path: PathBuf,
}

fn parse_ordinary_changed_entry(line: &[u8]) -> Result<Option<OrdinaryChangedEntry>> {
    if line.is_empty() {
        return Ok(None);
    }

    let mut fields = line.splitn(9, |&byte| byte == b' ');
    let marker = next_field(&mut fields)?;
    if marker != ORDINARY_CHANGED_ENTRY_MARKER.as_bytes() {
        return Ok(None);
    }

    let _xy = next_field(&mut fields)?;
    let _sub = next_field(&mut fields)?;
    let mode_head = str::from_utf8(next_field(&mut fields)?)?.to_owned();
    let mode_index = str::from_utf8(next_field(&mut fields)?)?.to_owned();
    let _mode_worktree = next_field(&mut fields)?;
    let hash_head = str::from_utf8(next_field(&mut fields)?)?.to_owned();
    let hash_index = str::from_utf8(next_field(&mut fields)?)?.to_owned();
    let path = PathBuf::from(str::from_utf8(next_field(&mut fields)?)?);

    Ok(Some(OrdinaryChangedEntry {
        mode_head,
        mode_index,
        hash_head,
        hash_index,
        path,
    }))
}

fn next_field<'a, I>(fields: &mut I) -> Result<&'a [u8]>
where
    I: Iterator<Item = &'a [u8]>,
{
    fields
        .next()
        .context("malformed `git status --porcelain=v2` output")
}

async fn is_destroyed_symlink(work_dir: &Path, entry: &OrdinaryChangedEntry) -> Result<bool> {
    if entry.hash_head == entry.hash_index {
        return Ok(true);
    }

    let size_index = git_object_size(work_dir, &entry.hash_index).await?;
    let size_head = git_object_size(work_dir, &entry.hash_head).await?;
    if size_index > size_head.saturating_add(2) {
        return Ok(false);
    }

    let head_content = git_object_content(work_dir, &entry.hash_head).await?;
    let index_content = git_object_content(work_dir, &entry.hash_index).await?;

    Ok(trim_trailing_ascii_whitespace(&head_content)
        == trim_trailing_ascii_whitespace(&index_content))
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

fn trim_trailing_ascii_whitespace(bytes: &[u8]) -> &[u8] {
    let end = bytes
        .iter()
        .rposition(|byte| !byte.is_ascii_whitespace())
        .map_or(0, |idx| idx + 1);
    &bytes[..end]
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

        assert_eq!(entry.mode_head, PERMS_LINK);
        assert_eq!(entry.mode_index, "100644");
        assert_eq!(entry.hash_head, "headhash");
        assert_eq!(entry.hash_index, "indexhash");
        assert_eq!(entry.path, Path::new("path with spaces.txt"));

        Ok(())
    }

    #[test]
    fn trim_trailing_ascii_whitespace_matches_upstream_behavior() {
        assert_eq!(trim_trailing_ascii_whitespace(b"target"), b"target");
        assert_eq!(trim_trailing_ascii_whitespace(b"target\n"), b"target");
        assert_eq!(trim_trailing_ascii_whitespace(b"target\r\n"), b"target");
        assert_eq!(trim_trailing_ascii_whitespace(b"target \t"), b"target");
        assert_eq!(trim_trailing_ascii_whitespace(b"\n\r\t "), b"");
    }
}
