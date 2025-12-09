// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

// Adapted from https://crates.io/crates/yaml-merge-keys to remove `yaml-rust2` from dependency.

use serde_yaml::{Mapping, Sequence, Value};

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
