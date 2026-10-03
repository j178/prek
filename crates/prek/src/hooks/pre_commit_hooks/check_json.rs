use std::borrow::Cow;
use std::path::Path;

use anyhow::Result;
use rustc_hash::FxHashSet;
use serde::{Deserialize, Deserializer};

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{FilenamesArgs, parse_hook_args, run_blocking_file_checks};

/// Runs the `check-json` hook.
pub(crate) async fn run(hook: &Hook, filenames: &[&Path]) -> Result<HookOutput> {
    let args: FilenamesArgs = parse_hook_args(hook)?;
    run_blocking_file_checks(
        hook.project().relative_path(),
        &args.filenames,
        filenames,
        check_file,
    )
    .await
}

fn check_file(file_path: &Path, display_path: &Path) -> Result<HookOutput> {
    let content = fs_err::read(file_path)?;
    let content = match simdutf8::compat::from_utf8(&content) {
        Ok(content) => content,
        Err(error) => {
            let error_message = format!(
                "{}: Failed to decode UTF-8 ({error})\n",
                display_path.display()
            );
            return Ok(HookOutput::unchanged(1, error_message.into_bytes()));
        }
    };

    let mut deserializer = serde_json::Deserializer::from_str(content);
    deserializer.disable_recursion_limit();
    let stacker = serde_stacker::Deserializer::new(&mut deserializer);

    match JsonDuplicateKeyChecker::deserialize(stacker).and_then(|_| deserializer.end()) {
        Ok(()) => Ok(HookOutput::unchanged(0, Vec::new())),
        Err(e) => {
            let error_message =
                format!("{}: Failed to json decode ({e})\n", display_path.display());
            Ok(HookOutput::unchanged(1, error_message.into_bytes()))
        }
    }
}

// Bare `Cow<str>` deserializes into an owned string. Borrow unescaped keys from the input.
#[derive(Deserialize)]
#[serde(transparent)]
struct JsonKey<'a>(#[serde(borrow)] Cow<'a, str>);

pub(crate) struct JsonDuplicateKeyChecker;

impl<'de> Deserialize<'de> for JsonDuplicateKeyChecker {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        use serde::de::{self, DeserializeSeed, MapAccess, SeqAccess, Visitor};
        use std::fmt;

        struct JsonDuplicateKeyVisitor<'de> {
            spare_keys: Vec<FxHashSet<Cow<'de, str>>>,
        }

        impl<'de> DeserializeSeed<'de> for &mut JsonDuplicateKeyVisitor<'de> {
            type Value = JsonDuplicateKeyChecker;

            fn deserialize<D>(self, deserializer: D) -> Result<Self::Value, D::Error>
            where
                D: Deserializer<'de>,
            {
                deserializer.deserialize_any(self)
            }
        }

        impl<'de> Visitor<'de> for &mut JsonDuplicateKeyVisitor<'de> {
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
                while seq.next_element_seed(&mut *self)?.is_some() {
                    // Keep traversing nested values to detect duplicate keys in objects.
                }
                Ok(JsonDuplicateKeyChecker)
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                let mut keys = self.spare_keys.pop().unwrap_or_default();
                while let Some(JsonKey(key)) = map.next_key::<JsonKey<'de>>()? {
                    if let Some(key) = keys.replace(key) {
                        return Err(de::Error::custom(format!("duplicate key `{key}`")));
                    }
                    map.next_value_seed(&mut *self)?;
                }
                // Reuse the allocation for sibling objects, but discard oversized tables.
                // Otherwise, clearing a large table for each small object can be quadratic.
                if keys.capacity() <= 4 * (keys.len() + 1) {
                    keys.clear();
                    self.spare_keys.push(keys);
                }
                Ok(JsonDuplicateKeyChecker)
            }
        }

        JsonDuplicateKeyVisitor {
            spare_keys: Vec::new(),
        }
        .deserialize(deserializer)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    #[test]
    fn escaped_keys_share_duplicate_detection_with_borrowed_keys() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("keys.json");
        for (content, status) in [
            (r#"{"key": 1, "\u006bey": 2}"#, 1),
            (r#"{"\u006bey": 1, "key": 2}"#, 1),
            (r#"{"😀": 1, "\ud83d\ude00": 2}"#, 1),
            (r#"{"a\nb": 1, "a\u000ab": 2}"#, 1),
            (r#"{"key": {"\u006bey": 1}, "keys": 2}"#, 0),
        ] {
            fs_err::write(&path, content)?;
            assert_eq!(check_file(&path, &path)?.exit_status, status, "{content}");
        }
        Ok(())
    }

    #[test]
    fn duplicate_keys_are_scoped_to_each_object() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("objects.json");
        for (content, status) in [
            (r#"[{"key": 1}, {"key": 2}]"#, 0),
            (r#"{"first": {"key": 1}, "second": {"\u006bey": 2}}"#, 0),
            (r#"{"key": 1, "nested": {"key": 2}, "key": 3}"#, 1),
            (r#"[{"key": 1}, {"nested": {"key": 1, "key": 2}}]"#, 1),
            (r#"[{"key": 1}, {"key": 2, "\u006bey": 3}]"#, 1),
        ] {
            fs_err::write(&path, content)?;
            assert_eq!(check_file(&path, &path)?.exit_status, status, "{content}");
        }
        Ok(())
    }

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
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");

        Ok(())
    }

    #[test]
    fn rejects_trailing_data() -> Result<()> {
        let dir = tempdir()?;
        let path = dir.path().join("trailing.json");
        for (content, status) in [
            ("{} trailing\n", 1),
            ("{\"a\": 1}\n{\"b\": 2}\n", 1),
            ("truefalse\n", 1),
            ("{} \t\r\n", 0),
        ] {
            fs_err::write(&path, content)?;
            assert_eq!(check_file(&path, &path)?.exit_status, status, "{content}");
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_json() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"{"key1": "value1", "key2": "value2""#;
        let file_path = create_test_file(&dir, "invalid.json", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"{"key1": "value1", "key1": "value2"}"#;
        let file_path = create_test_file(&dir, "duplicate.json", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_empty_json() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.json", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_utf8() -> Result<()> {
        let dir = tempdir()?;
        for content in [b"{\"key\":\"\xff\"}".as_slice(), b"{}\xff"] {
            let file_path = create_test_file(&dir, "invalid.json", content).await?;
            let result = check_file(&file_path, &file_path)?;
            assert_eq!(result.exit_status, 1);
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_valid_json_array() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"[{"key1": "value1"}, {"key2": "value2"}]"#;
        let file_path = create_test_file(&dir, "valid_array.json", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_duplicate_keys_in_nested_object() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"{"key1": "value1", "key2": {"nested_key": 1, "nested_key": 2}}"#;
        let file_path = create_test_file(&dir, "nested_duplicate.json", content).await?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");

        Ok(())
    }

    #[tokio::test]
    async fn test_recursion_limit() -> Result<()> {
        let dir = tempdir()?;

        for (open, close) in [("[", "]"), (r#"{"key":"#, "}")] {
            let json = format!("{}null{}", open.repeat(10000), close.repeat(10000));
            let file_path = create_test_file(&dir, "deeply_nested.json", json.as_bytes()).await?;
            let result = check_file(&file_path, &file_path)?;
            assert_eq!(result.exit_status, 0);
            assert_eq!(result.output, b"");
        }

        Ok(())
    }
}
