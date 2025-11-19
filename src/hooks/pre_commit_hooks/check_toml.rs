use std::path::Path;

use anyhow::Result;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

pub(crate) async fn check_toml(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| check_file(hook.project().relative_path(), filename).await)
        .buffered(*CONCURRENCY);

    let mut code = 0;
    let mut output = Vec::new();

    while let Some(result) = tasks.next().await {
        let (c, o) = result?;
        code |= c;
        output.extend(o);
    }

    Ok((code, output))
}

async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let content = fs_err::tokio::read(file_base.join(filename)).await?;
    if content.is_empty() {
        return Ok((0, Vec::new()));
    }

    // Use string content for borrowed parsing
    let content_str = match std::str::from_utf8(&content) {
        Ok(s) => s,
        Err(e) => {
            let error_message = format!("{}: Failed to decode UTF-8 ({e})\n", filename.display());
            return Ok((1, error_message.into_bytes()));
        }
    };

    // Use DeTable::parse_recoverable to report all parse errors at once
    let (_parsed, errors) = toml::de::DeTable::parse_recoverable(content_str);
    if errors.is_empty() {
        Ok((0, Vec::new()))
    } else {
        let mut error_messages = Vec::new();
        for error in errors {
            error_messages.push(format!(
                "{}: Failed to toml decode ({error})",
                filename.display()
            ));
        }
        let combined_errors = error_messages.join("\n") + "\n";
        Ok((1, combined_errors.into_bytes()))
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
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_valid_toml() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"key1 = "value1"
key2 = "value2"
"#;
        let file_path = create_test_file(&dir, "valid.toml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_toml() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"key1 = "value1
key2 = "value2"
"#;
        let file_path = create_test_file(&dir, "invalid.toml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"key1 = "value1"
key1 = "value2"
"#;
        let file_path = create_test_file(&dir, "duplicate.toml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_toml() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.toml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_multiple_errors_reported() -> Result<()> {
        let dir = tempdir()?;
        // TOML with multiple syntax errors
        let content = br#"key1 = "unclosed string
key2 = "value2"
key3 = invalid_value_without_quotes
[section
key4 = "another unclosed string
"#;
        let file_path = create_test_file(&dir, "multiple_errors.toml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);

        // Should contain multiple error messages (one for each error found)
        let error_count = output_str.matches("Failed to toml decode").count();
        assert!(error_count == 3, "Expected three errors, got: {output_str}");
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_utf8() -> Result<()> {
        let dir = tempdir()?;
        // Create content with invalid UTF-8 bytes
        let content = b"key1 = \"\xff\xfe\xfd\"\nkey2 = \"valid\"";
        let file_path = create_test_file(&dir, "invalid_utf8.toml", content).await?;

        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Failed to decode UTF-8"));
        assert!(output_str.contains("invalid_utf8.toml"));
        Ok(())
    }
}
