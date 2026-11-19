use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

use anyhow::Result;

use crate::hook::Hook;
use crate::hooks::{
    BINARY_PROBE_SIZE, FixArgs, HookOutput, looks_binary, parse_hook_args, run_blocking_file_checks,
};

/// Runs the `end-of-file-fixer` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: FixArgs = parse_hook_args(hook)?;
    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        move |file_path, display_path| fix_file(file_path, display_path, args.check),
    )
    .await
}

fn fix_file(file_path: &Path, display_path: &Path, check: bool) -> Result<HookOutput> {
    if fix_eof(file_path, check)? {
        let action = if check { "Would fix" } else { "Fixing" };
        Ok(HookOutput::known(
            1,
            format!("{action} {}\n", display_path.display()).into_bytes(),
            !check,
        ))
    } else {
        Ok(HookOutput::unchanged(0, Vec::new()))
    }
}

fn fix_eof(file_path: &Path, check: bool) -> Result<bool> {
    // If the file is empty, do nothing and avoid opening a write handle.
    let file_size = fs_err::metadata(file_path)?.len();
    if file_size == 0 {
        return Ok(false);
    }

    let mut file = fs_err::OpenOptions::new()
        .read(true)
        .write(!check)
        .open(file_path)?;

    let fixed_len = fixed_file_len(&mut file, file_size)?;
    if fixed_len == file_size {
        return Ok(false);
    }

    if !check {
        if fixed_len > file_size {
            file.seek(SeekFrom::Start(file_size))?;
            file.write_all(b"\n")?;
        } else {
            file.set_len(fixed_len)?;
        }
    }
    Ok(true)
}

/// Compute the target length for a newly opened file, including the binary-file check.
fn fixed_file_len(reader: &mut (impl Read + Seek), file_size: u64) -> Result<u64> {
    let mut buf = [0; BINARY_PROBE_SIZE];
    let fits_in_buffer = file_size <= buf.len() as u64;
    // Small files fit in the binary probe. Larger files often need only a content byte and CRLF.
    let mut len = if fits_in_buffer {
        usize::try_from(file_size)?
    } else {
        3
    };
    let mut offset = file_size - len as u64;

    let fixed_len = loop {
        if !fits_in_buffer {
            reader.seek(SeekFrom::Start(offset))?;
        }
        let tail = &mut buf[..len];
        reader.read_exact(tail)?;
        if let Some(len) = fixed_tail_len(tail) {
            break offset + len as u64;
        }
        if offset == 0 {
            return Ok(0);
        }

        // Overlap two bytes so the first CRLF after the body is in the same window.
        let end = offset + 2;
        offset = end.saturating_sub(buf.len() as u64);
        len = usize::try_from(end - offset)?;
    };

    if fixed_len == file_size {
        return Ok(file_size);
    }
    let prefix = if offset == 0 {
        // Any bytes beyond this window were already scanned and contained only CR/LF.
        &buf[..len]
    } else {
        reader.rewind()?;
        reader.read_exact(&mut buf)?;
        &buf[..]
    };
    Ok(if looks_binary(prefix) {
        file_size
    } else {
        fixed_len
    })
}

/// Keep the first trailing line ending, or make room for an LF if the body ends at EOF.
/// A window containing only CR/LF needs more context from the preceding bytes.
fn fixed_tail_len(tail: &[u8]) -> Option<usize> {
    let last = tail.iter().rposition(|b| !matches!(b, b'\r' | b'\n'))?;
    let body_len = last + 1;
    let ending_len = if tail[body_len..].starts_with(b"\r\n") {
        2
    } else {
        1
    };
    Some(body_len + ending_len)
}

#[cfg(test)]
mod tests {
    use super::*;

    use anyhow::Ok;
    use bstr::ByteSlice;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &[u8],
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_long_trailing_newlines_preserve_contents() -> Result<()> {
        let dir = tempdir()?;
        for ending in [b"\n".as_slice(), b"\r\n", b"\r"] {
            for count in [4095, 4096, 4097, 8193] {
                let content = [b"keep", ending.repeat(count).as_slice()].concat();
                let path = create_test_file(&dir, "long-tail.txt", &content).await?;

                let result = fix_file(&path, &path, true)?;
                assert_eq!(result.exit_status, 1);
                assert_eq!(fs_err::tokio::read(&path).await?, content);

                let result = fix_file(&path, &path, false)?;
                assert_eq!(result.exit_status, 1);
                assert_eq!(
                    fs_err::tokio::read(&path).await?,
                    [b"keep", ending].concat()
                );
            }
        }
        Ok(())
    }

    /// Preserve the body's CRLF even when the following blank lines use LF
    /// and the body and its terminator fall in different reads.
    #[tokio::test]
    async fn preserves_crlf_before_a_long_run_of_lfs() -> Result<()> {
        let dir = tempdir()?;
        let mut content = b"keep\r\n".to_vec();
        content.extend(std::iter::repeat_n(b'\n', 8192));
        let path = create_test_file(&dir, "mixed.txt", &content).await?;

        fix_file(&path, &path, false)?;
        assert_eq!(fs_err::read(path)?, b"keep\r\n");
        Ok(())
    }

    /// Appending a missing LF preserves the original bytes, including earlier line endings.
    #[tokio::test]
    async fn appends_lf_to_text_without_a_final_newline() -> Result<()> {
        let dir = tempdir()?;
        let cases: &[(&str, &[u8])] = &[
            ("lf.txt", b"one\ntwo"),
            ("crlf.txt", b"one\r\ntwo"),
            ("mixed.txt", b"one\r\ntwo\nthree"),
            ("utf8-bom.txt", "\u{feff}中文".as_bytes()),
            ("gbk.txt", b"\xd6\xd0\xce\xc4"), // 中文
        ];
        for &(name, contents) in cases {
            let path = create_test_file(&dir, name, contents).await?;
            let result = fix_file(&path, &path, false)?;

            assert_eq!(result.exit_status, 1, "{name}");
            assert!(result.output.as_bytes().contains_str("Fixing"), "{name}");
            assert_eq!(fs_err::read(&path)?, [contents, b"\n"].concat(), "{name}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_already_has_correct_windows_ending() -> Result<()> {
        let dir = tempdir()?;

        let content = b"line1\r\nline2\r\nline3\r\n";
        let file_path = create_test_file(&dir, "windows_with_eof.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 0, "Should not change the file");
        assert_eq!(result.output, b"");

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_already_has_correct_unix_ending() -> Result<()> {
        let dir = tempdir()?;

        let content = b"line1\nline2\nline3\n";
        let file_path = create_test_file(&dir, "unix_with_eof.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 0, "Should not change the file");
        assert_eq!(result.output, b"");

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content);

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<()> {
        let dir = tempdir()?;

        let content = b"";
        let file_path = create_test_file(&dir, "empty.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 0, "Should not change empty file");
        assert_eq!(result.output, b"");

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_excess_newlines_removal() -> Result<()> {
        let dir = tempdir()?;

        let content = b"line1\nline2\n\n\n\n";
        let file_path = create_test_file(&dir, "excess_newlines.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_excess_crlf_removal() -> Result<()> {
        let dir = tempdir()?;

        let content = b"line1\r\nline2\r\n\r\n\r\n";
        let file_path = create_test_file(&dir, "excess_crlf.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\r\nline2\r\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_all_newlines_make_empty() -> Result<()> {
        let dir = tempdir()?;

        let content = b"\n\n\n\n";
        let file_path = create_test_file(&dir, "only_newlines.txt", content).await?;

        let result = fix_file(&file_path, &file_path, false)?;

        assert_eq!(result.exit_status, 1, "Should fix the file");
        assert!(result.output.as_bytes().contains_str("Fixing"));

        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"");

        Ok(())
    }
}
