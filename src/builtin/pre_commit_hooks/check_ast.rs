use std::fmt::Write;
use std::path::Path;

use anyhow::Result;
use futures::StreamExt;
use ruff_python_parser::parse_module;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

pub(crate) async fn check_ast(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
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
    let content = fs_err::tokio::read_to_string(file_base.join(filename)).await?;

    let parse_result = parse_module(&content);

    match parse_result {
        Ok(parsed) => {
            let errors = parsed.errors();
            if errors.is_empty() {
                Ok((0, Vec::new()))
            } else {
                // Format error output similar to Python's traceback
                let mut error_output = format!("{}: failed parsing:\n", filename.display());

                for error in errors {
                    writeln!(error_output, "    {error}")?;
                }

                Ok((1, error_output.into_bytes()))
            }
        }
        Err(e) => {
            // Parse completely failed
            let error_message = format!("{}: failed parsing:\n    {}\n", filename.display(), e);
            Ok((1, error_message.into_bytes()))
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
        content: &str,
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_valid_python() -> Result<()> {
        let dir = tempdir()?;
        let content = r#"
def hello():
    print("Hello, world!")

if __name__ == "__main__":
    hello()
"#;
        let file_path = create_test_file(&dir, "valid.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_syntax_error() -> Result<()> {
        let dir = tempdir()?;
        let content = r#"
def hello()
    print("Missing colon")
"#;
        let file_path = create_test_file(&dir, "invalid.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("failed parsing"));
        Ok(())
    }

    #[tokio::test]
    async fn test_indentation_error() -> Result<()> {
        let dir = tempdir()?;
        let content = r#"
def hello():
print("Bad indentation")
"#;
        let file_path = create_test_file(&dir, "indent_error.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_file() -> Result<()> {
        let dir = tempdir()?;
        let content = "";
        let file_path = create_test_file(&dir, "empty.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_unclosed_string() -> Result<()> {
        let dir = tempdir()?;
        let content = r#"
message = "This string is not closed
print(message)
"#;
        let file_path = create_test_file(&dir, "unclosed_string.py", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }
}
