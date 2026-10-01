use std::path::{Path, PathBuf};

use anyhow::Result;
use bstr::ByteSlice;
use clap::{Parser, ValueEnum};
use memchr::memchr2;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{parse_hook_args, run_blocking_file_checks};

const CRLF: &[u8] = b"\r\n";
const LF: &[u8] = b"\n";
const CR: &[u8] = b"\r";

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Fix mixed line endings by converting to the most common line ending
    /// or a specified line ending.
    #[clap(long, short, value_enum, default_value_t = FixMode::Auto)]
    fix: FixMode,
    #[arg(value_name = "FILENAMES")]
    filenames: Vec<PathBuf>,
}

#[derive(Copy, Clone, Debug, Default, ValueEnum)]
#[allow(clippy::upper_case_acronyms)]
enum FixMode {
    /// Automatically determine the most common line ending and use it
    #[default]
    Auto,
    /// Don't fix, just report if mixed line endings are found
    No,
    /// Convert all line endings to LF
    LF,
    /// Convert all line endings to CRLF
    CRLF,
    /// Convert all line endings to CR
    CR,
}

#[derive(Default)]
struct LineEndingCounts {
    cr: usize,
    crlf: usize,
    lf: usize,
}

impl LineEndingCounts {
    fn kind_count(&self) -> usize {
        usize::from(self.cr > 0) + usize::from(self.crlf > 0) + usize::from(self.lf > 0)
    }

    fn has_any_except(&self, ending: &[u8]) -> bool {
        (ending != CR && self.cr > 0)
            || (ending != CRLF && self.crlf > 0)
            || (ending != LF && self.lf > 0)
    }
}

/// Runs the `mixed-line-ending` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: Args = parse_hook_args(hook)?;

    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        move |file_path, display_path| fix_file(file_path, display_path, args.fix),
    )
    .await
}

fn fix_file(file_path: &Path, display_path: &Path, fix_mode: FixMode) -> Result<HookOutput> {
    let contents = fs_err::read(file_path)?;

    // Without CR, auto/no/LF cannot change the file, including a partial final line.
    if matches!(fix_mode, FixMode::Auto | FixMode::No | FixMode::LF)
        && memchr::memchr(b'\r', &contents).is_none()
    {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    // Skip empty files or binary files
    if contents.is_empty() || contents.find_byte(0).is_some() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    let counts = count_line_endings(&contents);
    let has_mixed_endings = counts.kind_count() > 1;

    let target_ending = match fix_mode {
        FixMode::No => {
            return if has_mixed_endings {
                Ok(HookOutput::unchanged(
                    1,
                    format!("{}: mixed line endings\n", display_path.display()).into_bytes(),
                ))
            } else {
                Ok(HookOutput::unchanged(0, Vec::new()))
            };
        }
        FixMode::Auto => find_most_common_ending(&counts),
        FixMode::LF => LF,
        FixMode::CRLF => CRLF,
        FixMode::CR => CR,
    };
    if !counts.has_any_except(target_ending) {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    let contents = normalize_line_endings(contents, target_ending, &counts);
    fs_err::write(file_path, &contents)?;
    Ok(HookOutput::known(
        1,
        format!("Fixing {}\n", display_path.display()).into_bytes(),
        true,
    ))
}

fn count_line_endings(contents: &[u8]) -> LineEndingCounts {
    let Some((&first, rest)) = contents.split_first() else {
        return LineEndingCounts::default();
    };
    let mut cr = usize::from(first == b'\r');
    let mut lf = usize::from(first == b'\n');
    let mut crlf = 0;

    // Byte counters let the compiler compare and accumulate SIMD lanes without
    // widening every result. Reduce them before any lane can exceed u8::MAX.
    let batch_size = 64 * usize::from(u8::MAX);
    // The views stay one byte apart so CRLF pairs can cross block boundaries.
    for (batch, previous) in rest.chunks(batch_size).zip(contents.chunks(batch_size)) {
        let (blocks, tail) = batch.as_chunks::<64>();
        let mut cr_lanes = [0u8; 64];
        let mut lf_lanes = [0u8; 64];
        let mut crlf_lanes = [0u8; 64];
        for (block, previous) in blocks.iter().zip(previous.as_chunks::<64>().0) {
            for i in 0..64 {
                cr_lanes[i] += u8::from(block[i] == b'\r');
                lf_lanes[i] += u8::from(block[i] == b'\n');
                crlf_lanes[i] += u8::from(previous[i] == b'\r' && block[i] == b'\n');
            }
        }
        cr += cr_lanes.into_iter().map(usize::from).sum::<usize>();
        lf += lf_lanes.into_iter().map(usize::from).sum::<usize>();
        crlf += crlf_lanes.into_iter().map(usize::from).sum::<usize>();
        for (&byte, &previous) in tail.iter().zip(&previous[blocks.len() * 64..]) {
            cr += usize::from(byte == b'\r');
            lf += usize::from(byte == b'\n');
            crlf += usize::from(previous == b'\r' && byte == b'\n');
        }
    }

    LineEndingCounts {
        cr: cr - crlf,
        crlf,
        lf: lf - crlf,
    }
}

fn find_most_common_ending(counts: &LineEndingCounts) -> &'static [u8] {
    // Preserve the previous tie-break order from `[CR, CRLF, LF].max_by_key(...)`.
    if counts.lf >= counts.crlf && counts.lf >= counts.cr {
        LF
    } else if counts.crlf >= counts.cr {
        CRLF
    } else {
        CR
    }
}

fn normalize_line_endings(
    mut contents: Vec<u8>,
    ending: &[u8],
    counts: &LineEndingCounts,
) -> Vec<u8> {
    let needs_final_ending =
        !contents.is_empty() && !contents.ends_with(CR) && !contents.ends_with(LF);
    if ending == LF {
        let mut copied = 0;
        let mut written = 0;
        while let Some(offset) = memchr::memchr(b'\r', &contents[copied..]) {
            let index = copied + offset;
            let next = index + 1 + usize::from(contents.get(index + 1) == Some(&b'\n'));
            // LF conversion only shrinks the file, so unread bytes stay ahead of the output.
            contents.copy_within(copied..index, written);
            written += index - copied;
            contents[written] = b'\n';
            written += 1;
            copied = next;
        }
        contents.copy_within(copied.., written);
        contents.truncate(written + contents.len() - copied);
        if needs_final_ending {
            contents.push(b'\n');
        }
        return contents;
    }

    let endings = counts.cr + counts.crlf + counts.lf;
    let mut capacity = contents.len() - counts.crlf + endings * (ending.len() - 1);
    if needs_final_ending {
        capacity += ending.len();
    }
    let mut new_contents = Vec::with_capacity(capacity);
    let mut line_start = 0;
    let mut search_start = 0;

    while let Some(offset) = memchr2(b'\r', b'\n', &contents[search_start..]) {
        let index = search_start + offset;
        let ending_len = if contents[index] == b'\r' && contents.get(index + 1) == Some(&b'\n') {
            2
        } else {
            1
        };

        search_start = index + ending_len;
        if &contents[index..search_start] != ending {
            new_contents.extend_from_slice(&contents[line_start..index]);
            new_contents.extend_from_slice(ending);
            line_start = search_start;
        }
    }

    new_contents.extend_from_slice(&contents[line_start..]);
    if search_start < contents.len() {
        new_contents.extend_from_slice(ending);
    }

    new_contents
}

#[cfg(test)]
mod tests {
    use super::*;
    use bstr::ByteSlice;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn count_endings_across_blocks_and_counter_reductions() {
        for pattern in [b"x\r\n".as_slice(), b"\r", b"\n", b"\r\n", b"\r\r\n\n"] {
            let contents = pattern.repeat(33000);
            for start in 0..64 {
                for len in [0, 1, 2, 63, 64, 65, 16319, 16320, 16321, 32641] {
                    let contents = &contents[start..start + len];
                    let mut expected = (0, 0, 0);
                    let mut bytes = contents.iter().peekable();
                    while let Some(&byte) = bytes.next() {
                        match byte {
                            b'\r' if bytes.peek() == Some(&&b'\n') => {
                                expected.1 += 1;
                                bytes.next();
                            }
                            b'\r' => expected.0 += 1,
                            b'\n' => expected.2 += 1,
                            _ => {}
                        }
                    }
                    let counts = count_line_endings(contents);
                    assert_eq!((counts.cr, counts.crlf, counts.lf), expected);
                }
            }
        }
    }

    #[test]
    fn adjacent_endings_and_unterminated_tail() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("endings.txt");
        for (mode, expected) in [
            (FixMode::LF, b"one\n\ntwo\nlast\n".as_slice()),
            (FixMode::CRLF, b"one\r\n\r\ntwo\r\nlast\r\n"),
            (FixMode::CR, b"one\r\rtwo\rlast\r"),
            (FixMode::Auto, b"one\n\ntwo\nlast\n"),
        ] {
            fs_err::write(&path, b"one\r\r\ntwo\nlast")?;
            assert_eq!(fix_file(&path, &path, mode)?.exit_status, 1);
            assert_eq!(fs_err::read(&path)?, expected);
            assert_eq!(fix_file(&path, &path, mode)?.exit_status, 0);
        }
        Ok(())
    }

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
    async fn test_auto_fix_crlf_wins() -> Result<()> {
        let dir = tempdir()?;
        let content = b"line1\nline2\r\nline3\r\n"; // 1 LF, 2 CRLF
        let file_path = create_test_file(&dir, "mixed_crlf.txt", content).await?;
        let result = fix_file(&file_path, &file_path, FixMode::Auto)?;
        assert_eq!(result.exit_status, 1);
        assert!(result.output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\r\nline2\r\nline3\r\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_auto_fix_lf_wins() -> Result<()> {
        let dir = tempdir()?;
        let content = b"line1\nline2\nline3\r\n"; // 2 LF, 1 CRLF
        let file_path = create_test_file(&dir, "mixed_lf.txt", content).await?;
        let result = fix_file(&file_path, &file_path, FixMode::Auto)?;
        assert_eq!(result.exit_status, 1);
        assert!(result.output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\nline3\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_auto_fix_tie_prefers_lf() -> Result<()> {
        let dir = tempdir()?;
        let content = b"line1\nline2\r\n"; // 1 LF, 1 CRLF
        let file_path = create_test_file(&dir, "mixed_tie.txt", content).await?;
        let result = fix_file(&file_path, &file_path, FixMode::Auto)?;
        assert_eq!(result.exit_status, 1);
        assert!(result.output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\n");

        Ok(())
    }

    #[tokio::test]
    async fn test_fix_no() -> Result<()> {
        let dir = tempdir()?;
        let content = b"line1\nline2\r\n";
        let file_path = create_test_file(&dir, "mixed_no.txt", content).await?;
        let result = fix_file(&file_path, &file_path, FixMode::No)?;
        assert_eq!(result.exit_status, 1);
        assert!(result.output.as_bytes().contains_str("mixed line endings"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, content); // File should not be changed

        Ok(())
    }

    #[tokio::test]
    async fn test_no_line_endings() -> Result<()> {
        let dir = tempdir()?;
        let content = b"some content";
        let file_path = create_test_file(&dir, "no_endings.txt", content).await?;
        let result = fix_file(&file_path, &file_path, FixMode::Auto)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_fix_with_cr_endings() -> Result<()> {
        let dir = tempdir()?;
        // A file with a mix of all three line ending types
        let content = b"line1\rline2\nline3\r\n";
        let file_path = create_test_file(&dir, "all_mixed.txt", content).await?;

        // Test auto fix (should prefer LF as it's a 3-way tie)
        let result = fix_file(&file_path, &file_path, FixMode::Auto)?;
        assert_eq!(result.exit_status, 1);
        assert!(result.output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\nline2\nline3\n");

        // Restore content and test fix to CRLF
        fs_err::tokio::write(&file_path, content).await?;
        let result = fix_file(&file_path, &file_path, FixMode::CRLF)?;
        assert_eq!(result.exit_status, 1);
        assert!(result.output.as_bytes().contains_str("Fixing"));
        let new_content = fs_err::tokio::read(&file_path).await?;
        assert_eq!(new_content, b"line1\r\nline2\r\nline3\r\n");

        Ok(())
    }
}
