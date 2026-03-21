use std::path::Path;

use crate::hook::Hook;

pub(super) const ILLEGAL_WINDOWS_PATTERN: &str = r"(?i)((^|/)(CON|PRN|AUX|NUL|COM[\d\x{00B9}\x{00B2}\x{00B3}]|LPT[\d\x{00B9}\x{00B2}\x{00B3}])(\.|/|$)|[<>:\x22\\|?*\x00-\x1F]|/[^/]*[\.\s]/|[^/]*[\.\s]$)";

pub(crate) fn check_illegal_windows_names(_hook: &Hook, filenames: &[&Path]) -> (i32, Vec<u8>) {
    if filenames.is_empty() {
        return (0, Vec::new());
    }

    // Matching is handled by the hook's `files` pattern. Any filename that reaches this
    // builtin is already known to be invalid, so we only need to emit fail-style errors.
    let mut output = Vec::new();
    for filename in filenames {
        output.extend_from_slice(
            format!("{}: Illegal Windows filename\n", filename.display()).as_bytes(),
        );
    }

    (1, output)
}

#[cfg(test)]
mod tests {
    use super::ILLEGAL_WINDOWS_PATTERN;
    use fancy_regex::Regex;

    fn illegal_windows_re() -> Regex {
        Regex::new(ILLEGAL_WINDOWS_PATTERN).expect("illegal windows pattern must be valid")
    }

    #[test]
    fn test_legal_filename() {
        let re = illegal_windows_re();
        assert!(!re.is_match("normal_file.txt").unwrap());
        assert!(!re.is_match("src/main.rs").unwrap());
        assert!(!re.is_match("docs/README.md").unwrap());
    }

    #[test]
    fn test_reserved_names() {
        let re = illegal_windows_re();
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
        let re = illegal_windows_re();
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
        let re = illegal_windows_re();
        assert!(re.is_match("file.").unwrap());
        assert!(re.is_match("file ").unwrap());
        assert!(re.is_match("dir/file./next").unwrap());
    }
}
