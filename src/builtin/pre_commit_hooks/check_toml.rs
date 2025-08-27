use anyhow::Result;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

pub(crate) async fn check_toml(_hook: &Hook, filenames: &[&String]) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| check_file(filename).await)
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

async fn check_file(filename: &str) -> Result<(i32, Vec<u8>)> {
    let content = fs_err::tokio::read(filename).await?;
    if content.is_empty() {
        return Ok((0, Vec::new()));
    }

    match toml::from_slice::<toml::Value>(&content) {
        Ok(_) => Ok((0, Vec::new())),
        Err(e) => {
            let error_message = format!("{filename}: Failed to toml decode ({e})\n");
            Ok((1, error_message.into_bytes()))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use tempfile::tempdir;

    async fn create_test_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> PathBuf {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await.unwrap();
        file_path
    }

    async fn run_check_on_file(file_path: &Path) -> (i32, Vec<u8>) {
        let filename = file_path.to_string_lossy().to_string();
        check_file(&filename).await.unwrap()
    }

    #[tokio::test]
    async fn test_valid_toml() {
        let dir = tempdir().unwrap();
        let content = br#"key1 = "value1"
key2 = "value2"
"#;
        let file_path = create_test_file(&dir, "valid.toml", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 0);
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn test_invalid_toml() {
        let dir = tempdir().unwrap();
        let content = br#"key1 = "value1
key2 = "value2"
"#;
        let file_path = create_test_file(&dir, "invalid.toml", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
    }

    #[tokio::test]
    async fn test_duplicate_keys() {
        let dir = tempdir().unwrap();
        let content = br#"key1 = "value1"
key1 = "value2"
"#;
        let file_path = create_test_file(&dir, "duplicate.toml", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
    }

    #[tokio::test]
    async fn test_empty_toml() {
        let dir = tempdir().unwrap();
        let content = b"";
        let file_path = create_test_file(&dir, "empty.toml", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 0);
        assert!(output.is_empty());
    }
}
