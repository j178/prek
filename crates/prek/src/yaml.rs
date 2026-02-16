// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use std::fmt::Write;

/// Serialize a YAML scalar while preserving the caller's quote style.
pub(crate) fn serialize_yaml_scalar(value: &str, quote: &str) -> anyhow::Result<String> {
    match quote {
        "'" => Ok(format!("'{}'", escape_single_quoted(value))),
        "\"" => Ok(format!("\"{}\"", escape_double_quoted(value))),
        _ => {
            if is_simple_plain(value) {
                Ok(value.to_owned())
            } else {
                // Defer to serde-saphyr to select quoting/escaping for non-trivial scalars.
                let rendered = serde_saphyr::to_string(&value)?;
                Ok(rendered.trim_end_matches('\n').to_owned())
            }
        }
    }
}

/// Fast-path: allow simple, plain scalars we want to keep unquoted.
fn is_simple_plain(value: &str) -> bool {
    if value.is_empty() {
        return false;
    }
    value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '/' | '+' | '@'))
}

/// YAML single-quoted strings escape a single quote by doubling it.
fn escape_single_quoted(value: &str) -> String {
    value.replace('\'', "''")
}

/// YAML double-quoted strings use backslash escapes for control characters.
fn escape_double_quoted(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for ch in value.chars() {
        match ch {
            '\\' => escaped.push_str("\\\\"),
            '"' => escaped.push_str("\\\""),
            '\t' => escaped.push_str("\\t"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            c if c.is_control() => {
                let _ = write!(escaped, "\\u{:04X}", c as u32);
            }
            c => escaped.push(c),
        }
    }
    escaped
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
