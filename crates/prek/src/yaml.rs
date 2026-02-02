// Licensed under the Apache License, Version 2.0 <LICENSE-APACHE or
// http://www.apache.org/licenses/LICENSE-2.0> or the MIT license
// <LICENSE-MIT or http://opensource.org/licenses/MIT>, at your
// option. This file may not be copied, modified, or distributed
// except according to those terms.

use anyhow::Result;
use bstr::ByteSlice;
use libyaml::{Emitter, Encoding, Event, ScalarStyle};

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
