use std::path::Path;

use crate::hook::Hook;
use crate::hooks::pre_commit_hooks::check_json::JsonDuplicateKeyChecker;
use crate::hooks::{HookOutput, run_blocking_file_checks};

pub(crate) async fn check_json5(hook: &Hook, filenames: &[&Path]) -> anyhow::Result<HookOutput> {
    run_blocking_file_checks(hook.project().relative_path(), &[], filenames, check_file).await
}

fn check_file(file_path: &Path, display_path: &Path) -> anyhow::Result<HookOutput> {
    let content = fs_err::read_to_string(file_path)?;
    if content.is_empty() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    match json5::from_str::<JsonDuplicateKeyChecker>(&content) {
        Ok(_) => Ok(HookOutput::unchanged(0, Vec::new())),
        Err(e) => {
            let error_message = format!(
                "{}: Failed to json5 decode ({})\n",
                display_path.display(),
                e
            );
            Ok(HookOutput::unchanged(1, error_message.into_bytes()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &[u8],
    ) -> anyhow::Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::write(&file_path, content)?;
        Ok(file_path)
    }

    #[test]
    fn test_valid_json5() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let content = indoc::indoc! {r#"
        {
          // comments
          unquoted: "and you can quote me on that",
          singleQuotes: 'I can use "double quotes" here',
          lineBreaks: "Look, Mom! \
          No \\n's!",
          hexadecimal: 0xdecaf,
          leadingDecimalPoint: 0.8675309,
          andTrailing: 8675309,
          positiveSign: +1,
          trailingComma: "in objects",
          andIn: ["arrays"],
          backwardsCompatible: "with JSON",
        }
        "#};
        let file_path = create_test_file(&dir, "valid.json5", content.as_bytes())?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");

        Ok(())
    }

    #[test]
    fn test_duplicate_keys() -> anyhow::Result<()> {
        // JSON5 warns duplicate names are unpredictable; implementations may error or accept.
        // Our custom deserializer rejects duplicates.
        let dir = tempdir()?;
        let content = indoc::indoc! {r#"
        {
          key: "value1",
          key: "value2",
          key: "value3",
        }
        "#};
        let file_path = create_test_file(&dir, "duplicate.json5", content.as_bytes())?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert!(String::from_utf8_lossy(&result.output).contains("duplicate key"));

        Ok(())
    }

    #[test]
    fn test_invalid_json5() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "invalid.json5", b"{ key: 'value' ")?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");

        Ok(())
    }
}
