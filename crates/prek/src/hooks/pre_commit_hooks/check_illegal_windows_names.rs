use std::io::Write;
use std::path::Path;

use anyhow::Result;

use crate::hook::Hook;
use crate::hooks::HookOutput;

pub(crate) const ILLEGAL_WINDOWS_PATTERN: &str = r"(?i)((^|/)(CON|PRN|AUX|NUL|COM[\d\x{00B9}\x{00B2}\x{00B3}]|LPT[\d\x{00B9}\x{00B2}\x{00B3}])(\.|/|$)|[<>:\x22\\|?*\x00-\x1F]|/[^/]*[\.\s]/|[^/]*[\.\s]$)";

#[expect(
    clippy::unused_async,
    reason = "pre-commit hook runners share an async interface"
)]
pub(crate) async fn run(_hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    if filenames.is_empty() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    Ok(HookOutput::unchanged(
        1,
        illegal_windows_names_output(filenames),
    ))
}

fn illegal_windows_names_output(filenames: &[&Path]) -> Vec<u8> {
    let mut output = Vec::new();
    for filename in filenames {
        writeln!(output, "{}: Illegal Windows filename", filename.display())
            .expect("writing to Vec should never fail");
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;
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

    #[test]
    fn test_output_lines() {
        let filenames = [Path::new("CON.txt"), Path::new("bad:name.txt")];
        let output = illegal_windows_names_output(&filenames);
        assert_eq!(
            String::from_utf8(output).unwrap(),
            "CON.txt: Illegal Windows filename\nbad:name.txt: Illegal Windows filename\n"
        );
    }
}
