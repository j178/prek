use std::borrow::Cow;
use std::path::Path;

use anyhow::Result;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Deserializer};

use crate::hook::Hook;
use crate::hooks::run_concurrent_file_checks;
use crate::run::INTERNAL_CONCURRENCY;

pub(crate) async fn check_json(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    run_concurrent_file_checks(
        filenames.iter().copied(),
        *INTERNAL_CONCURRENCY,
        |filename| check_file(hook.project().relative_path(), filename),
    )
    .await
}

async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let file_path = file_base.join(filename);
    let content = fs_err::tokio::read(file_path).await?;
    if content.is_empty() {
        return Ok((0, Vec::new()));
    }

    let mut deserializer = serde_json::Deserializer::from_slice(&content);
    deserializer.disable_recursion_limit();
    let deserializer = serde_stacker::Deserializer::new(&mut deserializer);

    // Try to parse with duplicate key detection
    match JsonDuplicateKeyChecker::deserialize(deserializer) {
        Ok(JsonDuplicateKeyChecker) => Ok((0, Vec::new())),
        Err(e) => {
            let error_message = format!("{}: Failed to json decode ({e})\n", filename.display());
            Ok((1, error_message.into_bytes()))
        }
    }
}

pub(crate) struct JsonDuplicateKeyChecker;

impl<'de> Deserialize<'de> for JsonDuplicateKeyChecker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, MapAccess, SeqAccess, Visitor};
        use std::fmt;

        struct JsonDuplicateKeyVisitor;

        impl<'de> Visitor<'de> for JsonDuplicateKeyVisitor {
            type Value = JsonDuplicateKeyChecker;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a JSON value")
            }

            fn visit_bool<E>(self, _v: bool) -> Result<Self::Value, E> {
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_i64<E>(self, _v: i64) -> Result<Self::Value, E> {
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_u64<E>(self, _v: u64) -> Result<Self::Value, E> {
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_f64<E>(self, _v: f64) -> Result<Self::Value, E> {
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_str<E>(self, _v: &str) -> Result<Self::Value, E> {
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_string<E>(self, _v: String) -> Result<Self::Value, E> {
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_unit<E>(self) -> Result<Self::Value, E> {
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                while seq.next_element::<JsonDuplicateKeyChecker>()?.is_some() {
                    // Keep traversing nested values to detect duplicate keys in objects.
                }
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut keys = FxHashSet::default();
                while let Some(key) = map.next_key::<Cow<'de, str>>()? {
                    if keys.contains(&key) {
                        return Err(de::Error::custom(format!("duplicate key `{key}`")));
                    }
                    map.next_value::<JsonDuplicateKeyChecker>()?;
                    keys.insert(key);
                }
                Ok(JsonDuplicateKeyChecker)
            }
        }

        deserializer.deserialize_any(JsonDuplicateKeyVisitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
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
    async fn test_valid_json() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"{"key1": "value1", "key2": "value2"}"#;
        let file_path = create_test_file(&dir, "valid.json", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_json() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"{"key1": "value1", "key2": "value2""#;
        let file_path = create_test_file(&dir, "invalid.json", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"{"key1": "value1", "key1": "value2"}"#;
        let file_path = create_test_file(&dir, "duplicate.json", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_json() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.json", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_valid_json_array() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"[{"key1": "value1"}, {"key2": "value2"}]"#;
        let file_path = create_test_file(&dir, "valid_array.json", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys_in_nested_object() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"{"key1": "value1", "key2": {"nested_key": 1, "nested_key": 2}}"#;
        let file_path = create_test_file(&dir, "nested_duplicate.json", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());

        Ok(())
    }

    #[tokio::test]
    async fn test_recursion_limit() -> Result<()> {
        let dir = tempdir()?;

        let mut json = String::new();
        for _ in 0..10000 {
            json = format!("[{json}]");
        }

        let file_path = create_test_file(&dir, "deeply_nested.json", json.as_bytes()).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());

        Ok(())
    }
}
