// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use anyhow::Result;
use bstr::ByteSlice;
use libyaml::{Emitter, Encoding, Event, ScalarStyle};
use serde_yaml::{Mapping, Sequence, Value};

/// Serialize a YAML scalar while preserving the caller's quote style.
pub(crate) fn serialize_yaml_scalar(value: &str, quote: &str) -> Result<String> {
    let style = match quote {
        "'" => Some(ScalarStyle::SingleQuoted),
        "\"" => Some(ScalarStyle::DoubleQuoted),
        _ => None,
    };

    let mut writer = Vec::new();
    {
        let mut emitter = Emitter::new(&mut writer)?;
        emitter.emit(Event::StreamStart {
            encoding: Some(Encoding::Utf8),
        })?;
        emitter.emit(Event::DocumentStart {
            version: None,
            tags: vec![],
            implicit: true,
        })?;
        emitter.emit(Event::Scalar {
            anchor: None,
            tag: None,
            value: value.to_owned(),
            plain_implicit: true,
            quoted_implicit: true,
            style,
        })?;
        emitter.emit(Event::DocumentEnd { implicit: true })?;
        emitter.emit(Event::StreamEnd {})?;
        emitter.flush()?;
    }
    let trimmed = writer.trim_end();
    Ok(str::from_utf8(trimmed)?.to_owned())
}

// Adapted from https://crates.io/crates/yaml-merge-keys to remove `yaml-rust2` from dependency.

/// Errors which may occur when performing the YAML merge key process.
///
/// This enum is `non_exhaustive`, but cannot be marked as such until it is stable. In the
/// meantime, there is a hidden variant.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum MergeKeyError {
    /// A non-hash value was given as a value to merge into a hash.
    ///
    /// This happens with a document such as:
    ///
    /// ```yaml
    /// -
    ///   <<: 4
    ///   x: 1
    /// ```
    #[error("only mappings and arrays of mappings may be merged")]
    InvalidMergeValue,
}

/// Merge two hashes together.
fn merge_hashes(mut hash: Mapping, rhs: Mapping) -> Mapping {
    rhs.into_iter().for_each(|(key, value)| {
        hash.entry(key).or_insert(value);
    });
    hash
}

/// Merge values together.
fn merge_values(hash: Mapping, value: Value) -> Result<Mapping, MergeKeyError> {
    let merge_values = match value {
        Value::Sequence(arr) => {
            let init: Result<Mapping, _> = Ok(Mapping::new());

            arr.into_iter().fold(init, |res_hash, item| {
                // Merge in the next item.
                res_hash.and_then(move |res_hash| {
                    if let Value::Mapping(next_hash) = item {
                        Ok(merge_hashes(res_hash, next_hash))
                    } else {
                        // Non-hash values at this level are not allowed.
                        Err(MergeKeyError::InvalidMergeValue)
                    }
                })
            })?
        }
        Value::Mapping(merge_hash) => merge_hash,
        _ => return Err(MergeKeyError::InvalidMergeValue),
    };

    Ok(merge_hashes(hash, merge_values))
}

/// Recurse into a hash and handle items with merge keys in them.
fn merge_hash(hash: Mapping) -> Result<Value, MergeKeyError> {
    let mut hash = hash
        .into_iter()
        // First handle any merge keys in the key or value...
        .map(|(key, value)| {
            merge_keys(key).and_then(|key| merge_keys(value).map(|value| (key, value)))
        })
        .collect::<Result<Mapping, _>>()?;

    if let Some(merge_value) = hash.remove("<<") {
        merge_values(hash, merge_value).map(Value::Mapping)
    } else {
        Ok(Value::Mapping(hash))
    }
}

/// Recurse into an array and handle items with merge keys in them.
fn merge_array(arr: Sequence) -> Result<Value, MergeKeyError> {
    arr.into_iter()
        .map(merge_keys)
        .collect::<Result<Sequence, _>>()
        .map(Value::Sequence)
}

/// Handle merge keys in a YAML document.
pub fn merge_keys(doc: Value) -> Result<Value, MergeKeyError> {
    match doc {
        Value::Mapping(hash) => merge_hash(hash),
        Value::Sequence(arr) => merge_array(arr),
        _ => Ok(doc),
    }
}

#[cfg(test)]
mod tests {
    use super::merge_keys;
    use super::serialize_yaml_scalar;

    #[test]
    fn serialize_yaml_scalar_plain() {
        let rendered = serialize_yaml_scalar("v1.2.3", "").unwrap();
        assert_eq!(rendered, "v1.2.3");
        let rendered = serialize_yaml_scalar("v1.2.3", "'").unwrap();
        assert_eq!(rendered, "'v1.2.3'");
        let rendered = serialize_yaml_scalar("v1.2.3", "\"").unwrap();
        assert_eq!(rendered, "\"v1.2.3\"");
        let rendered = serialize_yaml_scalar("123", "").unwrap();
        assert_eq!(rendered, "123");
        let rendered = serialize_yaml_scalar("123", "'").unwrap();
        assert_eq!(rendered, "'123'");
        let rendered = serialize_yaml_scalar("123", "\"").unwrap();
        assert_eq!(rendered, "\"123\"");
        let rendered = serialize_yaml_scalar("a:b", "'").unwrap();
        assert_eq!(rendered, "'a:b'");
        let rendered = serialize_yaml_scalar("a\"b", "\"").unwrap();
        assert_eq!(rendered, "\"a\\\"b\"");
        let rendered = serialize_yaml_scalar("a'b", "'").unwrap();
        assert_eq!(rendered, "'a''b'");
    }

    type Yaml = serde_yaml::Value;

    fn yaml_null() -> Yaml {
        Yaml::Null
    }

    fn yaml_boolean(b: bool) -> Yaml {
        Yaml::Bool(b)
    }

    fn yaml_string(s: &'static str) -> Yaml {
        Yaml::String(s.into())
    }

    fn yaml_integer(i: i64) -> Yaml {
        Yaml::Number(i.into())
    }

    fn yaml_real(_: &'static str, r: f64) -> Yaml {
        Yaml::Number(r.into())
    }

    fn yaml_array(ts: Vec<Yaml>) -> Yaml {
        Yaml::Sequence(ts)
    }

    fn yaml_hash(ts: impl Iterator<Item = (Yaml, Yaml)>) -> Yaml {
        Yaml::Mapping(ts.collect())
    }
    fn assert_yaml_idempotent(doc: &Yaml) {
        assert_eq!(&merge_keys(doc.clone()).unwrap(), doc);
    }

    fn merge_key() -> Yaml {
        yaml_string("<<")
    }

    macro_rules! yaml_hash {
    [ $( $pair:expr ),* $(,)? ] => {
        yaml_hash([$( $pair, )*].iter().cloned())
    };
}

    #[test]
    fn test_ignore_non_containers() {
        let null = yaml_null();
        let bool_true = yaml_boolean(true);
        let bool_false = yaml_boolean(false);
        let string = yaml_string("");
        let integer = yaml_integer(1234);
        let real = yaml_real("0.02", 0.02);

        assert_yaml_idempotent(&null);
        assert_yaml_idempotent(&bool_true);
        assert_yaml_idempotent(&bool_false);
        assert_yaml_idempotent(&string);
        assert_yaml_idempotent(&integer);
        assert_yaml_idempotent(&real);
    }

    #[test]
    fn test_ignore_container_no_merge_keys() {
        let arr = yaml_array(vec![yaml_integer(10), yaml_integer(100)]);
        let hash = yaml_hash![
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
        ];

        assert_yaml_idempotent(&arr);
        assert_yaml_idempotent(&hash);
    }

    #[test]
    fn test_remove_merge_keys() {
        let hash = yaml_hash![
            (merge_key(), yaml_hash![]),
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
        ];
        let expected = yaml_hash![
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
        ];

        assert_eq!(merge_keys(hash).unwrap(), expected);
    }

    #[test]
    fn test_handle_merge_keys() {
        let hash = yaml_hash![
            (merge_key(), yaml_hash![(yaml_integer(15), yaml_null())]),
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
        ];
        let expected = yaml_hash![
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
            (yaml_integer(15), yaml_null()),
        ];

        assert_eq!(merge_keys(hash).unwrap(), expected);
    }

    #[test]
    fn test_merge_key_precedence() {
        let hash = yaml_hash![
            (
                merge_key(),
                yaml_hash![(yaml_integer(10), yaml_integer(10))],
            ),
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
        ];
        let expected = yaml_hash![
            (yaml_integer(100), yaml_string("string")),
            (yaml_integer(10), yaml_null()),
        ];

        assert_eq!(merge_keys(hash).unwrap(), expected);
    }

    #[test]
    fn test_merge_key_array() {
        let hash = yaml_hash![
            (
                merge_key(),
                yaml_array(vec![
                    yaml_hash![(yaml_integer(15), yaml_integer(10))],
                    yaml_hash![(yaml_integer(20), yaml_integer(10))],
                ]),
            ),
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
        ];
        let expected = yaml_hash![
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
            (yaml_integer(15), yaml_integer(10)),
            (yaml_integer(20), yaml_integer(10)),
        ];

        assert_eq!(merge_keys(hash).unwrap(), expected);
    }

    #[test]
    fn test_merge_key_array_precedence() {
        let hash = yaml_hash![
            (
                merge_key(),
                yaml_array(vec![
                    yaml_hash![(yaml_integer(15), yaml_integer(10))],
                    yaml_hash![(yaml_integer(15), yaml_integer(20))],
                ]),
            ),
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
        ];
        let expected = yaml_hash![
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
            (yaml_integer(15), yaml_integer(10)),
        ];

        assert_eq!(merge_keys(hash).unwrap(), expected);
    }

    #[test]
    fn test_merge_key_nested_array() {
        let hash = yaml_array(vec![yaml_hash![
            (
                merge_key(),
                yaml_array(vec![
                    yaml_hash![(yaml_integer(15), yaml_integer(10))],
                    yaml_hash![(yaml_integer(15), yaml_integer(20))],
                ]),
            ),
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
        ]]);
        let expected = yaml_array(vec![yaml_hash![
            (yaml_integer(10), yaml_null()),
            (yaml_integer(100), yaml_string("string")),
            (yaml_integer(15), yaml_integer(10)),
        ]]);

        assert_eq!(merge_keys(hash).unwrap(), expected);
    }

    #[test]
    fn test_merge_key_nested_hash_value() {
        let hash = yaml_hash![(
            yaml_null(),
            yaml_hash![
                (
                    merge_key(),
                    yaml_array(vec![
                        yaml_hash![(yaml_integer(15), yaml_integer(10))],
                        yaml_hash![(yaml_integer(15), yaml_integer(20))],
                    ]),
                ),
                (yaml_integer(10), yaml_null()),
                (yaml_integer(100), yaml_string("string")),
            ],
        )];
        let expected = yaml_hash![(
            yaml_null(),
            yaml_hash![
                (yaml_integer(10), yaml_null()),
                (yaml_integer(100), yaml_string("string")),
                (yaml_integer(15), yaml_integer(10)),
            ],
        )];

        assert_eq!(merge_keys(hash).unwrap(), expected);
    }

    #[test]
    fn test_merge_key_nested_hash_key() {
        let hash = yaml_hash![(
            yaml_hash![
                (
                    merge_key(),
                    yaml_array(vec![
                        yaml_hash![(yaml_integer(15), yaml_integer(10))],
                        yaml_hash![(yaml_integer(15), yaml_integer(20))],
                    ]),
                ),
                (yaml_integer(10), yaml_null()),
                (yaml_integer(100), yaml_string("string")),
            ],
            yaml_null(),
        )];
        let expected = yaml_hash![(
            yaml_hash![
                (yaml_integer(10), yaml_null()),
                (yaml_integer(100), yaml_string("string")),
                (yaml_integer(15), yaml_integer(10)),
            ],
            yaml_null(),
        )];

        assert_eq!(merge_keys(hash).unwrap(), expected);
    }

    #[test]
    fn test_yaml_spec_examples() {
        let center = yaml_hash![
            (yaml_string("x"), yaml_integer(1)),
            (yaml_string("y"), yaml_integer(2)),
        ];
        let left = yaml_hash![
            (yaml_string("x"), yaml_integer(0)),
            (yaml_string("y"), yaml_integer(2)),
        ];
        let big = yaml_hash![(yaml_string("r"), yaml_integer(10))];
        let small = yaml_hash![(yaml_string("r"), yaml_integer(1))];

        let explicit = yaml_hash![
            (yaml_string("x"), yaml_integer(1)),
            (yaml_string("y"), yaml_integer(2)),
            (yaml_string("r"), yaml_integer(10)),
            (yaml_string("label"), yaml_string("center/big")),
        ];
        let explicit_ordered = yaml_hash![
            (yaml_string("r"), yaml_integer(10)),
            (yaml_string("label"), yaml_string("center/big")),
            (yaml_string("x"), yaml_integer(1)),
            (yaml_string("y"), yaml_integer(2)),
        ];
        let explicit_ordered_multiple = yaml_hash![
            (yaml_string("label"), yaml_string("center/big")),
            (yaml_string("x"), yaml_integer(1)),
            (yaml_string("y"), yaml_integer(2)),
            (yaml_string("r"), yaml_integer(10)),
        ];
        let merge_one_map = yaml_hash![
            (merge_key(), center.clone()),
            (yaml_string("r"), yaml_integer(10)),
            (yaml_string("label"), yaml_string("center/big")),
        ];
        let merge_multiple_maps = yaml_hash![
            (merge_key(), yaml_array(vec![center, big.clone()])),
            (yaml_string("r"), yaml_integer(10)),
            (yaml_string("label"), yaml_string("center/big")),
        ];
        let overrides = yaml_hash![
            (merge_key(), yaml_array(vec![big, left, small])),
            (yaml_string("x"), yaml_integer(1)),
            (yaml_string("label"), yaml_string("center/big")),
        ];

        assert_eq!(merge_keys(explicit.clone()).unwrap(), explicit);
        assert_eq!(merge_keys(merge_one_map).unwrap(), explicit_ordered);
        assert_eq!(
            merge_keys(merge_multiple_maps).unwrap(),
            explicit_ordered_multiple,
        );
        assert_eq!(merge_keys(overrides).unwrap(), explicit_ordered_multiple);
    }

    macro_rules! assert_is_error {
        ( $doc:expr, $kind:path ) => {
            let _ = merge_keys($doc).unwrap_err();

            /* XXX: irrefutable
            if let $kind = err {
                // Expected error.
            } else {
                panic!("unexpected error: {:?}", err);
            }
            */
        };
    }

    #[test]
    fn test_invalid_merge_key_values() {
        let merge_null = yaml_hash![(merge_key(), yaml_null())];
        let merge_bool = yaml_hash![(merge_key(), yaml_boolean(false))];
        let merge_string = yaml_hash![(merge_key(), yaml_string(""))];
        let merge_integer = yaml_hash![(merge_key(), yaml_integer(0))];
        let merge_real = yaml_hash![(merge_key(), yaml_real("0.02", 0.02))];

        assert_is_error!(merge_null, MergeKeyError::InvalidMergeValue);
        assert_is_error!(merge_bool, MergeKeyError::InvalidMergeValue);
        assert_is_error!(merge_string, MergeKeyError::InvalidMergeValue);
        assert_is_error!(merge_integer, MergeKeyError::InvalidMergeValue);
        assert_is_error!(merge_real, MergeKeyError::InvalidMergeValue);
    }

    #[test]
    fn test_invalid_merge_key_array_values() {
        let merge_null = yaml_hash![(merge_key(), yaml_array(vec![yaml_null()]))];
        let merge_bool = yaml_hash![(merge_key(), yaml_array(vec![yaml_boolean(false)]))];
        let merge_string = yaml_hash![(merge_key(), yaml_array(vec![yaml_string("")]))];
        let merge_integer = yaml_hash![(merge_key(), yaml_array(vec![yaml_integer(0)]))];
        let merge_real = yaml_hash![(merge_key(), yaml_array(vec![yaml_real("0.02", 0.02)]))];

        assert_is_error!(merge_null, MergeKeyError::InvalidMergeValue);
        assert_is_error!(merge_bool, MergeKeyError::InvalidMergeValue);
        assert_is_error!(merge_string, MergeKeyError::InvalidMergeValue);
        assert_is_error!(merge_integer, MergeKeyError::InvalidMergeValue);
        assert_is_error!(merge_real, MergeKeyError::InvalidMergeValue);
    }
}
