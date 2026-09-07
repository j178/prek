use std::path::Path;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::check_json::JsonDuplicateKeyChecker;
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;

pub(crate) async fn check_jsonc(hook: &Hook, filenames: &[&Path]) -> anyhow::Result<HookOutput> {
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

    let options = jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas: true,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    };
    match jsonc_parser::parse_to_serde_value::<JsonDuplicateKeyChecker>(&content, &options) {
        Ok(_) => Ok(HookOutput::unchanged(0, Vec::new())),
        Err(e) => {
            let error_message = format!("{}: Failed to jsonc decode ({e})\n", filename.display());
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
    async fn test_valid_jsonc() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let content = indoc::indoc! {r#"
        {
          // single-line comments
          "a": "b",  // postfix comments
          "c": "d",
          /*
            multi
            line
            comments
          */
          "e": /* inline comments */ "f",
          "url": "https://example.com/*not-a-comment*/",
          "objects": [{"key": 1}, {"key": 2}]
        }
        "#};
        let file_path = create_test_file(&dir, "valid.jsonc", content.as_bytes()).await?;
        let result = check_file(dir.path(), &file_path).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_valid_jsonc_trailing_comma() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let content = indoc::indoc! {r#"
        {
          "array": [1,2,3,],
          "object": {"k": "v",}
        }
        "#};
        let file_path = create_test_file(&dir, "valid.jsonc", content.as_bytes()).await?;
        let result = check_file(dir.path(), &file_path).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys() -> anyhow::Result<()> {
        let dir = tempdir()?;
        for content in [
            r#"{"key": 1, /* comment */ "key": 2}"#,
            r#"[{"nested": {"key": 1, "key": 2}}]"#,
            r#"{"key": 1, "\u006bey": 2}"#,
        ] {
            let file_path = create_test_file(&dir, "duplicate.jsonc", content.as_bytes()).await?;
            let result = check_file(dir.path(), &file_path).await?;
            assert_eq!(result.exit_status, 1, "input: {content:?}");
            assert!(String::from_utf8_lossy(&result.output).contains("duplicate key"));
        }

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_jsonc() -> anyhow::Result<()> {
        let dir = tempdir()?;
        for content in [
            r#"{key: "value"}"#,
            r#"{"key": 'value'}"#,
            r#"{"key": 0x10}"#,
            r#"{"key": +1}"#,
            r#"{"key": .5}"#,
            r#"{"key": 1.}"#,
            r#"{"key": NaN}"#,
            r#"{"key": Infinity}"#,
            r#"{"a": 1 "b": 2}"#,
            "[1 2]",
            "[01]",
            "[1,,]",
            "[,]",
            "{,}",
            "{} {}",
            "{} garbage",
            "{} /* unterminated",
            "# comment\n{}",
        ] {
            let file_path = create_test_file(&dir, "invalid.jsonc", content.as_bytes()).await?;
            let result = check_file(dir.path(), &file_path).await?;
            assert_eq!(result.exit_status, 1, "input: {content:?}");
        }

        Ok(())
    }
}
