use anyhow::Result;
use futures::StreamExt;
use serde::Deserialize;
use serde_with::{MapPreventDuplicates, serde_as};
use std::collections::HashMap;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

#[serde_as]
#[derive(Deserialize)]
#[serde(untagged)]
enum JsonValue {
    Object(#[serde_as(as = "MapPreventDuplicates<_, _>")] HashMap<String, JsonValue>),
    Array(Vec<JsonValue>),
    String(String),
    Number(serde_json::Number),
    Bool(bool),
    Null,
}

pub(crate) async fn check_json(_hook: &Hook, filenames: &[&String]) -> Result<(i32, Vec<u8>)> {
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

    // Parse JSON with duplicate key detection using serde_with
    let result: Result<JsonValue, _> = serde_json::from_slice(&content);
    match result {
        Ok(_) => Ok((0, Vec::new())),
        Err(e) => {
            let error_message = if e.to_string().contains("duplicate") {
                format!("{filename}: {e}\n")
            } else {
                format!("{filename}: Failed to json decode ({e})\n")
            };
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
    async fn test_valid_json() {
        let dir = tempdir().unwrap();
        let content = br#"{"key1": "value1", "key2": "value2"}"#;
        let file_path = create_test_file(&dir, "valid.json", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 0);
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn test_invalid_json() {
        let dir = tempdir().unwrap();
        let content = br#"{"key1": "value1", "key2": "value2""#;
        let file_path = create_test_file(&dir, "invalid.json", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
    }

    #[tokio::test]
    async fn test_duplicate_keys() {
        let dir = tempdir().unwrap();
        let content = br#"{"key1": "value1", "key1": "value2"}"#;
        let file_path = create_test_file(&dir, "duplicate.json", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
    }

    #[tokio::test]
    async fn test_empty_json() {
        let dir = tempdir().unwrap();
        let content = b"";
        let file_path = create_test_file(&dir, "empty.json", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 0);
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn test_valid_json_array() {
        let dir = tempdir().unwrap();
        let content = br#"[{"key1": "value1"}, {"key2": "value2"}]"#;
        let file_path = create_test_file(&dir, "valid_array.json", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 0);
        assert!(output.is_empty());
    }

    #[tokio::test]
    async fn test_duplicate_keys_in_nested_object() {
        let dir = tempdir().unwrap();
        let content = br#"{"key1": "value1", "key2": {"nested_key": 1, "nested_key": 2}}"#;
        let file_path = create_test_file(&dir, "nested_duplicate.json", content).await;
        let (code, output) = run_check_on_file(&file_path).await;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
    }
}
