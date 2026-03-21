use std::path::Path;

use anyhow::Result;
use fancy_regex::Regex;
use std::sync::LazyLock;

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::run::CONCURRENCY;

/// Matches GitHub blob URLs that use a branch name instead of a commit hash.
/// A permalink uses a hex commit hash (4-64 chars) after `/blob/`.
/// A non-permalink uses a branch name (not all-hex, or shorter than 4 chars).
static NON_PERMALINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"https://github\.com/[^/ ]+/[^/ ]+/blob/(?![a-fA-F0-9]{4,64}/)([^/. ]+)/[^# ]+#L\d+",
    )
    .expect("Invalid regex")
});

pub(crate) async fn check_vcs_permalinks(
    hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let file_base = hook.project().relative_path();
    run_concurrent_file_checks(filenames.iter().copied(), *CONCURRENCY, |filename| {
        check_file(file_base, filename)
    })
    .await
}

async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let path = file_base.join(filename);
    let content = match fs_err::tokio::read(&path).await {
        Ok(c) => c,
        Err(_) => return Ok((0, Vec::new())),
    };

    let mut retval = 0;
    let mut output = Vec::new();

    for (i, line) in content.split(|&b| b == b'\n').enumerate() {
        let line_str = String::from_utf8_lossy(line);
        if NON_PERMALINK_RE.is_match(&line_str)? {
            retval = 1;
            output.extend_from_slice(
                format!("{}:{}:{}\n", filename.display(), i + 1, line_str).as_bytes(),
            );
        }
    }

    if retval != 0 {
        output.extend_from_slice(b"\nNon-permanent GitHub link detected.\n");
        output.extend_from_slice(b"On any page on GitHub press [y] to load a permalink.\n");
    }

    Ok((retval, output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_permalink_not_flagged() {
        let re = &*NON_PERMALINK_RE;
        // Commit hash permalink - should NOT match
        assert!(!re.is_match("https://github.com/owner/repo/blob/abc123def456/file.py#L10").unwrap());
        assert!(!re.is_match("https://github.com/owner/repo/blob/abcdef1234567890abcdef1234567890abcdef12/src/main.rs#L42").unwrap());
    }

    #[test]
    fn test_branch_link_flagged() {
        let re = &*NON_PERMALINK_RE;
        // Branch name links - SHOULD match
        assert!(re.is_match("https://github.com/owner/repo/blob/main/file.py#L10").unwrap());
        assert!(re.is_match("https://github.com/owner/repo/blob/master/src/lib.rs#L5").unwrap());
        assert!(re.is_match("https://github.com/owner/repo/blob/develop/README.md#L1").unwrap());
    }

    #[test]
    fn test_no_line_number_not_flagged() {
        let re = &*NON_PERMALINK_RE;
        // No line number anchor - should NOT match (the hook only flags links with #L)
        assert!(!re.is_match("https://github.com/owner/repo/blob/main/file.py").unwrap());
    }
}
