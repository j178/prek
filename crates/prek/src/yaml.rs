// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use serde_saphyr::{DoubleQuoted, SingleQuoted};

/// Serialize a YAML scalar while preserving the caller's quote style.
pub(crate) fn serialize_yaml_scalar(value: &str, quote: &str) -> anyhow::Result<String> {
    let mut rendered = match quote {
        "'" => serde_saphyr::to_string(&SingleQuoted(value))?,
        "\"" => serde_saphyr::to_string(&DoubleQuoted(value))?,
        _ => serde_saphyr::to_string(&value)?,
    };

    if rendered.ends_with('\n') {
        rendered.pop();
    }
    Ok(rendered)
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
        assert_eq!(rendered, "\"123\"");
        let rendered = serialize_yaml_scalar("2", "").unwrap();
        assert_eq!(rendered, "\"2\"");
        let rendered = serialize_yaml_scalar("0.49", "").unwrap();
        assert_eq!(rendered, "\"0.49\"");
        let rendered = serialize_yaml_scalar("yes", "").unwrap();
        assert_eq!(rendered, "\"yes\"");
        let rendered = serialize_yaml_scalar("123", "'").unwrap();
        assert_eq!(rendered, "'123'");
        let rendered = serialize_yaml_scalar("123", "\"").unwrap();
        assert_eq!(rendered, "\"123\"");
        let rendered = serialize_yaml_scalar("a:b", "").unwrap();
        assert_eq!(rendered, "a:b");
        let rendered = serialize_yaml_scalar("a:b", "'").unwrap();
        assert_eq!(rendered, "'a:b'");
        let rendered = serialize_yaml_scalar("a\"b", "\"").unwrap();
        assert_eq!(rendered, "\"a\\\"b\"");
        let rendered = serialize_yaml_scalar("a'b", "'").unwrap();
        assert_eq!(rendered, "'a''b'");

        let rendered = serialize_yaml_scalar("abc def", "").unwrap();
        assert_eq!(rendered, "abc def");
        let rendered = serialize_yaml_scalar("abc def", "'").unwrap();
        assert_eq!(rendered, "'abc def'");
        let rendered = serialize_yaml_scalar("abc def", "\"").unwrap();
        assert_eq!(rendered, "\"abc def\"");
    }

    #[test]
    fn serialize_yaml_scalar_quotes_and_escapes() {
        let rendered = serialize_yaml_scalar("a\\b", "\"").unwrap();
        assert_eq!(rendered, "\"a\\\\b\"");
        let rendered = serialize_yaml_scalar("a\nb", "\"").unwrap();
        assert_eq!(rendered, "\"a\\nb\"");
        let rendered = serialize_yaml_scalar("a\tb", "\"").unwrap();
        assert_eq!(rendered, "\"a\\tb\"");
        let rendered = serialize_yaml_scalar("a\\b", "'").unwrap();
        assert_eq!(rendered, "'a\\b'");
    }
}
