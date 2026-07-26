use std::path::Path;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::check_json::JsonDuplicateKeyChecker;
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;

pub(crate) async fn check_json5(hook: &Hook, filenames: &[&Path]) -> anyhow::Result<HookOutput> {
    run_concurrent_file_checks(
        filenames.iter().copied(),
        *INTERNAL_CONCURRENCY,
        |filename| check_file(hook.project().relative_path(), filename),
    )
    .await
}

async fn check_file(file_base: &Path, filename: &Path) -> anyhow::Result<HookOutput> {
    let file_path = file_base.join(filename);
    let content = fs_err::tokio::read_to_string(file_path).await?;
    if content.is_empty() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    match json5::from_str::<JsonDuplicateKeyChecker>(&content) {
        Ok(_) => Ok(HookOutput::unchanged(0, Vec::new())),
        Err(e) => {
            let error_message = format!("{}: Failed to json5 decode ({})\n", filename.display(), e);
            Ok(HookOutput::unchanged(1, error_message.into_bytes()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &[u8],
    ) -> anyhow::Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_valid_json5() -> anyhow::Result<()> {
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
        let file_path = create_test_file(&dir, "valid.json5", content.as_bytes()).await?;
        let result = check_file(dir.path(), &file_path).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys() -> anyhow::Result<()> {
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
        let file_path = create_test_file(&dir, "duplicate.json5", content.as_bytes()).await?;
        let result = check_file(dir.path(), &file_path).await?;
        assert_eq!(result.exit_status, 1);
        assert!(String::from_utf8_lossy(&result.output).contains("duplicate key"));

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_json5() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "invalid.json5", b"{ key: 'value' ").await?;
        let result = check_file(dir.path(), &file_path).await?;
        assert_eq!(result.exit_status, 1);
        assert!(!result.output.is_empty());

        Ok(())
    }
}
