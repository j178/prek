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
    if entry.is_empty() {
        return None;
    }

    let tab_index = entry.iter().position(|&byte| byte == b'\t')?;
    let (metadata, file_name) = entry.split_at(tab_index);
    let file_name = Path::new(str::from_utf8(&file_name[1..]).ok()?);
    let file_name = file_name.strip_prefix(file_base).unwrap_or(file_name);
    if !filenames.contains(file_name) {
        return None;
    }

    let mode_bits = parse_mode_bits(metadata)?;
    (((mode_bits & 0o111) != 0) == executable).then_some(file_name)
}

fn parse_mode_bits(metadata: &[u8]) -> Option<u32> {
    let mode = metadata.split(|&byte| byte == b' ').next()?;
    if mode.is_empty() {
        return None;
    }

    let mut mode_bits = 0;
    for &byte in mode {
        if !(b'0'..=b'7').contains(&byte) {
            return None;
        }
        mode_bits = (mode_bits << 3) + u32::from(byte - b'0');
    }
    Some(mode_bits)
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

    #[test]
    fn parse_mode_bits_reads_octal_prefix() {
        assert_eq!(parse_mode_bits(b"100755 abcdef 0"), Some(0o100_755));
        assert_eq!(parse_mode_bits(b"100644"), Some(0o100_644));
        assert_eq!(parse_mode_bits(b""), None);
        assert_eq!(parse_mode_bits(b"100888 abcdef 0"), None);
    }

    #[tokio::test]
    async fn file_has_shebang_detects_valid_shebang() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        fs_err::tokio::write(file.path(), b"#!/bin/sh\necho hi\n").await?;

        assert!(file_has_shebang(file.path()).await?);
        Ok(())
    }

    #[tokio::test]
    async fn file_has_shebang_rejects_non_shebang_prefixes() -> Result<(), anyhow::Error> {
        let file = NamedTempFile::new()?;
        fs_err::tokio::write(file.path(), b"##!/bin/sh\n").await?;

        assert!(!file_has_shebang(file.path()).await?);
        Ok(())
    }
}
