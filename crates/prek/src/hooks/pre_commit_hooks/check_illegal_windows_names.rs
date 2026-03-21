use std::path::Path;
use std::sync::LazyLock;

use anyhow::Result;
use fancy_regex::Regex;

use crate::hook::Hook;

static ILLEGAL_WINDOWS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)((^|/)(CON|PRN|AUX|NUL|COM[\d\x{00B9}\x{00B2}\x{00B3}]|LPT[\d\x{00B9}\x{00B2}\x{00B3}])(\.|/|$)|[<>:\x22\\|?*\x00-\x1F]|/[^/]*[\.\s]/|[^/]*[\.\s]$)",
    )
    .expect("Invalid regex")
});

pub(crate) async fn check_illegal_windows_names(
    _hook: &Hook,
    filenames: &[&Path],
) -> Result<(i32, Vec<u8>)> {
    let mut retval = 0;
    let mut output = Vec::new();

    for filename in filenames {
        let filename_str = filename.to_string_lossy();
        if ILLEGAL_WINDOWS_RE.is_match(&filename_str)? {
            retval = 1;
            output.extend_from_slice(
                format!("{}: Illegal Windows filename\n", filename.display()).as_bytes(),
            );
        }
    }

    Ok((retval, output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_legal_filename() {
        let re = &*ILLEGAL_WINDOWS_RE;
        assert!(!re.is_match("normal_file.txt").unwrap());
        assert!(!re.is_match("src/main.rs").unwrap());
        assert!(!re.is_match("docs/README.md").unwrap());
    }

    #[test]
    fn test_reserved_names() {
        let re = &*ILLEGAL_WINDOWS_RE;
        assert!(re.is_match("CON").unwrap());
        assert!(re.is_match("PRN").unwrap());
        assert!(re.is_match("AUX").unwrap());
        assert!(re.is_match("NUL").unwrap());
        assert!(re.is_match("COM1").unwrap());
        assert!(re.is_match("LPT1").unwrap());
        assert!(re.is_match("con").unwrap());
        assert!(re.is_match("CON.txt").unwrap());
        assert!(re.is_match("dir/CON/file").unwrap());
    }

    #[test]
    fn test_illegal_characters() {
        let re = &*ILLEGAL_WINDOWS_RE;
        assert!(re.is_match("file<name").unwrap());
        assert!(re.is_match("file>name").unwrap());
        assert!(re.is_match("file:name").unwrap());
        assert!(re.is_match("file\"name").unwrap());
        assert!(re.is_match("file|name").unwrap());
        assert!(re.is_match("file?name").unwrap());
        assert!(re.is_match("file*name").unwrap());
    }

    #[test]
    fn test_trailing_dot_or_space() {
        let re = &*ILLEGAL_WINDOWS_RE;
        assert!(re.is_match("file.").unwrap());
        assert!(re.is_match("file ").unwrap());
        assert!(re.is_match("dir/file./next").unwrap());
    }
}
