use std::path::Path;
use std::str;

use rustc_hash::FxHashSet;
use tokio::io::AsyncReadExt;

use crate::git;

pub(super) async fn file_has_shebang(path: &Path) -> Result<bool, anyhow::Error> {
    let mut file = fs_err::tokio::File::open(path).await?;
    let mut buf = [0u8; 2];
    let n = file.read(&mut buf).await?;
    Ok(n >= 2 && buf[0] == b'#' && buf[1] == b'!')
}

pub(super) async fn git_index_stage_output(file_base: &Path) -> Result<Vec<u8>, anyhow::Error> {
    Ok(git::git_cmd("git ls-files")?
        .arg("ls-files")
        .arg("--stage")
        .arg("-z")
        .arg("--")
        .arg(if file_base.as_os_str().is_empty() {
            Path::new(".")
        } else {
            file_base
        })
        .check(true)
        .output()
        .await?
        .stdout)
}

pub(super) fn matching_git_index_paths_by_executable_bit<'a>(
    stdout: &'a [u8],
    file_base: &'a Path,
    filenames: &'a FxHashSet<&Path>,
    executable: bool,
) -> impl Iterator<Item = &'a Path> + 'a {
    stdout
        .split(|&b| b == b'\0')
        .filter_map(move |entry| parse_stage_entry(entry, file_base, filenames, executable))
}

fn parse_stage_entry<'a>(
    entry: &'a [u8],
    file_base: &Path,
    filenames: &FxHashSet<&Path>,
    executable: bool,
) -> Option<&'a Path> {
    let entry = str::from_utf8(entry).ok()?;
    if entry.is_empty() {
        return None;
    }

    let (metadata, file_name) = entry.split_once('\t')?;
    let file_name = Path::new(file_name);
    let file_name = file_name.strip_prefix(file_base).unwrap_or(file_name);
    if !filenames.contains(file_name) {
        return None;
    }

    let mode_bits = u32::from_str_radix(metadata.split_whitespace().next()?, 8).ok()?;
    (((mode_bits & 0o111) != 0) == executable).then_some(file_name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    #[test]
    fn parse_stage_entry_strips_project_prefix() {
        let filenames = FxHashSet::from_iter([Path::new("script.sh")]);
        let entry = b"100644 abcdef0123456789abcdef0123456789abcdef 0\tsubdir/script.sh";

        assert_eq!(
            parse_stage_entry(entry, Path::new("subdir"), &filenames, false),
            Some(Path::new("script.sh"))
        );
    }

    #[test]
    fn parse_stage_entry_filters_by_executable_bit() {
        let filenames = FxHashSet::from_iter([Path::new("script.sh")]);
        let executable_entry = b"100755 abcdef0123456789abcdef0123456789abcdef 0\tscript.sh";
        let non_executable_entry = b"100644 abcdef0123456789abcdef0123456789abcdef 0\tscript.sh";

        assert_eq!(
            parse_stage_entry(executable_entry, Path::new(""), &filenames, true),
            Some(Path::new("script.sh"))
        );
        assert_eq!(
            parse_stage_entry(executable_entry, Path::new(""), &filenames, false),
            None
        );
        assert_eq!(
            parse_stage_entry(non_executable_entry, Path::new(""), &filenames, false),
            Some(Path::new("script.sh"))
        );
    }

    #[tokio::test]
    async fn file_has_shebang_detects_valid_shebang() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"#!/bin/sh\necho hi\n").await?;

        assert!(file_has_shebang(file.path()).await?);
        Ok(())
    }

    #[tokio::test]
    async fn file_has_shebang_rejects_non_shebang_prefixes() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        tokio::fs::write(file.path(), b"##!/bin/sh\n").await?;

        assert!(!file_has_shebang(file.path()).await?);
        Ok(())
    }
}
