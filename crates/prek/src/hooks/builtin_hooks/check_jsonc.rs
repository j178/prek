use std::path::Path;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::check_json::JsonDuplicateKeyChecker;
use crate::hooks::pre_commit_hooks::parse_hook_args;
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;
use clap::Parser;

#[derive(Parser)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
pub(crate) struct Args {
    /// Allow trailing commas in objects and arrays.
    #[arg(short = 't', long)]
    allow_trailing_commas: bool,
}

pub(crate) async fn check_jsonc(hook: &Hook, filenames: &[&Path]) -> anyhow::Result<HookOutput> {
    let args = parse_hook_args::<Args>(hook)?;
    run_concurrent_file_checks(
        filenames.iter().copied(),
        *INTERNAL_CONCURRENCY,
        |filename| {
            check_file(
                hook.project().relative_path(),
                filename,
                args.allow_trailing_commas,
            )
        },
    )
    .await
}

async fn check_file(
    file_base: &Path,
    filename: &Path,
    allow_trailing_commas: bool,
) -> anyhow::Result<HookOutput> {
    let file_path = file_base.join(filename);
    let content = fs_err::tokio::read_to_string(file_path).await?;
    if content.is_empty() {
        return Ok(HookOutput::unchanged(0, Vec::new()));
    }

    let options = jsonc_parser::ParseOptions {
        allow_comments: true,
        allow_loose_object_property_names: false,
        allow_trailing_commas,
        allow_missing_commas: false,
        allow_single_quoted_strings: false,
        allow_hexadecimal_numbers: false,
        allow_unary_plus_numbers: false,
    };
    match jsonc_parser::parse_to_serde_value::<JsonDuplicateKeyChecker>(&content, &options) {
        Ok(_) => Ok(HookOutput::unchanged(0, Vec::new())),
        Err(e) => {
            let error_message = format!("{}: Failed to jsonc decode ({})\n", filename.display(), e);
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
          "e": /* inline comments */ "f"
        }
        "#};
        let file_path = create_test_file(&dir, "valid.jsonc", content.as_bytes()).await?;
        let result = check_file(dir.path(), &file_path, false).await?;
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
        let result = check_file(dir.path(), &file_path, true).await?;
        assert_eq!(result.exit_status, 0);
        assert!(result.output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let content = indoc::indoc! {r#"
        {
          "key": "value1",
          "key": "value2"
        }
        "#};
        let file_path = create_test_file(&dir, "duplicate.jsonc", content.as_bytes()).await?;
        let result = check_file(dir.path(), &file_path, false).await?;
        assert_eq!(result.exit_status, 1);
        assert!(String::from_utf8_lossy(&result.output).contains("duplicate key"));

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_jsonc() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "invalid.jsonc", b"{ key: 'value' ").await?;
        let result = check_file(dir.path(), &file_path, true).await?;
        assert_eq!(result.exit_status, 1);
        assert!(!result.output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_jsonc_trailing_comma() -> anyhow::Result<()> {
        let dir = tempdir()?;
        let file_path =
            create_test_file(&dir, "trailing_comma.jsonc", b"{ \"key\": \"value\", }").await?;
        let result1 = check_file(dir.path(), &file_path, false).await?;
        assert_eq!(result1.exit_status, 1);
        assert!(!result1.output.is_empty());

        let result2 = check_file(dir.path(), &file_path, true).await?;
        assert_eq!(result2.exit_status, 0);
        assert!(result2.output.is_empty());

        Ok(())
    }
}
