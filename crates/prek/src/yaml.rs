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
}
