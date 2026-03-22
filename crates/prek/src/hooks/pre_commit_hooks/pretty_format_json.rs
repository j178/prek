use std::fmt::Write;
use std::io;
use std::path::Path;

use anyhow::Result;
use clap::Parser;
use futures::StreamExt;
use owo_colors::OwoColorize;
use serde::Serialize;
use serde_json::ser::{Formatter, PrettyFormatter};
use serde_json::{Map, Value};
use similar::{ChangeTag, TextDiff};

use crate::hook::Hook;
use crate::run::CONCURRENCY;

#[derive(Parser, Debug)]
#[command(disable_help_subcommand = true)]
#[command(disable_version_flag = true)]
#[command(disable_help_flag = true)]
struct Args {
    #[arg(long)]
    autofix: bool,

    #[arg(long, default_value = "2")]
    indent: String,

    #[arg(long = "no-ensure-ascii")]
    no_ensure_ascii: bool,

    #[arg(long = "no-sort-keys")]
    no_sort_keys: bool,

    #[arg(long = "top-keys", value_delimiter = ',')]
    top_keys: Vec<String>,
}

impl Args {
    fn indent(&self) -> Vec<u8> {
        match self.indent.parse::<usize>() {
            Ok(num_spaces) => vec![b' '; num_spaces],
            Err(_) => self.indent.as_bytes().to_vec(),
        }
    }
}

pub(crate) async fn pretty_format_json(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let args = Args::try_parse_from(hook.entry.resolve(None)?.iter().chain(&hook.args))?;
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| check_file(hook.project().relative_path(), filename, &args).await)
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

async fn check_file(file_base: &Path, filename: &Path, args: &Args) -> Result<(i32, Vec<u8>)> {
    let original_content = fs_err::tokio::read_to_string(file_base.join(filename)).await?;
    if original_content.is_empty() {
        let error_message = format!(
            "{}: Failed to json parse (no element found). Think about using the 'check-json' hook. \n",
            filename.display()
        );
        return Ok((1, error_message.into_bytes()));
    }

    match prettify_json(&original_content, args) {
        Ok(prettified_json) => {
            if original_content == prettified_json {
                Ok((0, Vec::new()))
            } else if args.autofix {
                fs_err::tokio::write(file_base.join(filename), prettified_json.as_bytes()).await?;
                let message = format!("Fixed & autoformatted file {} \n", filename.display());
                Ok((1, message.into_bytes()))
            } else {
                let diff_output = get_diff(
                    &original_content,
                    &prettified_json,
                    filename.to_str().unwrap(),
                );
                let message = format!(
                    "File {}: is not pretty-formatted.\n{}",
                    filename.display(),
                    diff_output
                );
                Ok((1, message.into_bytes()))
            }
        }
        Err(e) => {
            let error_message = format!(
                "{}: Failed to json parse: {}. Think about using the 'check-json' hook. \n",
                filename.display(),
                e
            );
            Ok((1, error_message.into_bytes()))
        }
    }
}

fn prettify_json(json: &str, args: &Args) -> Result<String> {
    let mut value: Value = serde_json::from_str(json)?;
    value = reorder_keys(value, &args.top_keys, !args.no_sort_keys);

    let indent_bytes = args.indent();
    let mut buf = Vec::with_capacity(json.len());
    let formatter = JsonFormatter::with_indent(&indent_bytes, !args.no_ensure_ascii);
    let mut ser = serde_json::Serializer::with_formatter(&mut buf, formatter);
    value.serialize(&mut ser)?;

    let mut result = String::from_utf8(buf)?;
    // Always end with exactly one newline
    if !result.ends_with('\n') {
        result.push('\n');
    }
    Ok(result)
}

struct JsonFormatter<'a> {
    pretty: PrettyFormatter<'a>,
    ensure_ascii: bool,
}

impl<'a> JsonFormatter<'a> {
    fn with_indent(indent: &'a [u8], ensure_ascii: bool) -> Self {
        // `serde_json` does not expose an `ensure_ascii` option, so we reuse its
        // pretty-printer state and only customize string fragment emission.
        Self {
            pretty: PrettyFormatter::with_indent(indent),
            ensure_ascii,
        }
    }
}

impl Formatter for JsonFormatter<'_> {
    fn begin_array<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_array(writer)
    }

    fn end_array<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_array(writer)
    }

    fn begin_array_value<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_array_value(writer, first)
    }

    fn end_array_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_array_value(writer)
    }

    fn begin_object<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object(writer)
    }

    fn end_object<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_object(writer)
    }

    fn begin_object_key<W>(&mut self, writer: &mut W, first: bool) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object_key(writer, first)
    }

    fn begin_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.begin_object_value(writer)
    }

    fn end_object_value<W>(&mut self, writer: &mut W) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        self.pretty.end_object_value(writer)
    }

    fn write_string_fragment<W>(&mut self, writer: &mut W, fragment: &str) -> io::Result<()>
    where
        W: ?Sized + io::Write,
    {
        if !self.ensure_ascii || fragment.is_ascii() {
            return writer.write_all(fragment.as_bytes());
        }

        write_ascii_only_fragment(writer, fragment)
    }
}

fn write_ascii_only_fragment<W>(writer: &mut W, fragment: &str) -> io::Result<()>
where
    W: ?Sized + io::Write,
{
    let mut start = 0;

    for (index, ch) in fragment.char_indices() {
        if ch.is_ascii() {
            continue;
        }

        if start < index {
            writer.write_all(&fragment.as_bytes()[start..index])?;
        }
        write_unicode_escape(writer, ch)?;
        start = index + ch.len_utf8();
    }

    writer.write_all(&fragment.as_bytes()[start..])
}

fn write_unicode_escape<W>(writer: &mut W, ch: char) -> io::Result<()>
where
    W: ?Sized + io::Write,
{
    let mut buf = [0_u16; 2];
    for unit in ch.encode_utf16(&mut buf).iter().copied() {
        write_u16_escape(writer, unit)?;
    }
    Ok(())
}

fn write_u16_escape<W>(writer: &mut W, unit: u16) -> io::Result<()>
where
    W: ?Sized + io::Write,
{
    const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

    let escape = [
        b'\\',
        b'u',
        HEX_DIGITS[((unit >> 12) & 0x0f) as usize],
        HEX_DIGITS[((unit >> 8) & 0x0f) as usize],
        HEX_DIGITS[((unit >> 4) & 0x0f) as usize],
        HEX_DIGITS[(unit & 0x0f) as usize],
    ];
    writer.write_all(&escape)
}

/// Recursively reorder JSON object keys with optional top-level keys and sorting.
///
/// Reorder keys according to the following rules:
/// 1. Keys specified in `top_keys` are placed first, in the order they appear in the slice
/// 2. Remaining keys are either sorted alphabetically (if `sort_keys` is true) or kept in original order
/// 3. The reordering is applied recursively to all nested objects and arrays
fn reorder_keys(mut value: Value, top_keys: &[String], sort_keys: bool) -> Value {
    match &mut value {
        Value::Object(map) => {
            let mut new_map = Map::new();

            for key in top_keys {
                if let Some(v) = map.remove(key) {
                    new_map.insert(key.clone(), reorder_keys(v, top_keys, sort_keys));
                }
            }

            let mut remaining: Vec<_> = map.iter_mut().collect();
            if sort_keys {
                remaining.sort_by_key(|(k, _)| *k);
            }

            for (k, v) in remaining {
                new_map.insert(k.clone(), reorder_keys(v.take(), top_keys, sort_keys));
            }

            Value::Object(new_map)
        }

        // Recursively process arrays
        Value::Array(arr) => {
            let new_arr: Vec<Value> = arr
                .drain(..)
                .map(|v| reorder_keys(v, top_keys, sort_keys))
                .collect();
            Value::Array(new_arr)
        }
        _ => value,
    }
}

fn get_diff(original: &str, formatted: &str, filename: &str) -> String {
    let diff = TextDiff::from_lines(original, formatted);

    let mut output = String::new();
    writeln!(output, "{}", format!("--- {filename}").bold()).unwrap();
    writeln!(output, "{}", format!("+++ {filename}").bold()).unwrap();

    for change in diff.iter_all_changes() {
        let line = match change.tag() {
            ChangeTag::Delete => format!("-{change}").red().to_string(),
            ChangeTag::Insert => format!("+{change}").green().to_string(),
            ChangeTag::Equal => format!(" {change}").to_string(),
        };
        output.push_str(&line);
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    const PRETTY_JSON: &str = r#"{
  "alist": [
    2,
    34,
    234
  ],
  "blah": null,
  "foo": "bar"
}
"#;

    const UNSORTED_JSON: &str = r#"{
  "foo": "bar",
  "alist": [
    2,
    34,
    234
  ],
  "blah": null
}
"#;

    const NON_ASCII_JSON: &str = r#"{
  "alist": [
    2,
    34,
    234
  ],
  "blah": null,
  "foo": "bar",
  "non_ascii": "\u4E2D\u6587\u306B\u307B\u3093\u3054\uD55C\uAD6D\uC5B4"
}
"#;

    async fn create_test_file(
        dir: &tempfile::TempDir,
        name: &str,
        content: &str,
    ) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::tokio::write(&file_path, content).await?;
        Ok(file_path)
    }

    #[tokio::test]
    async fn test_empty_json_file() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "empty.json", "").await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: false,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&output).contains("Failed to json parse"));
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_json() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "invalid.json", r#"{"foo": bar}"#).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: false,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&output).contains("Failed to json parse"));
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_pretty_json_file() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "pretty.json", PRETTY_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: false,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 0);
        assert!(output.is_empty());
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_unsorted_json_file() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "non_pretty.json", UNSORTED_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: false,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("is not pretty-formatted"));
        assert!(output_str.contains("-  \"foo\": \"bar\""));
        assert!(output_str.contains("+  \"foo\": \"bar\""));
        assert!(output_str.contains("+  \"blah\": null,"));
        assert!(output_str.contains("-  \"blah\": null"));
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_sorting_disabled() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "non_pretty.json", UNSORTED_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: false,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: true,
                top_keys: vec![],
            },
        )
        .await?;

        // With sorting disabled, no changes needed
        assert_eq!(code, 0);
        assert!(output.is_empty());
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_top_keys() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "non_pretty.json", UNSORTED_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: false,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec!["blah".to_string()],
            },
        )
        .await?;

        assert_eq!(code, 1);
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("is not pretty-formatted"));
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_autofix() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "non_pretty.json", UNSORTED_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&output).contains("Fixed & autoformatted file"));

        // Verify the file was actually fixed
        let result = fs_err::tokio::read_to_string(&file_path).await?;
        assert_eq!(result, PRETTY_JSON);
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_tab_indent() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "non_pretty.json", UNSORTED_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "\t".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&output).contains("Fixed & autoformatted file"));

        let result = fs_err::tokio::read_to_string(&file_path).await?;
        let expected = "{\n\t\"alist\": [\n\t\t2,\n\t\t34,\n\t\t234\n\t],\n\t\"blah\": null,\n\t\"foo\": \"bar\"\n}\n";
        assert_eq!(result, expected);
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_custom_space_indent() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "non_pretty.json", UNSORTED_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "4".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&output).contains("Fixed & autoformatted file"));

        let result = fs_err::tokio::read_to_string(&file_path).await?;
        let expected = r#"{
    "alist": [
        2,
        34,
        234
    ],
    "blah": null,
    "foo": "bar"
}
"#;
        assert_eq!(result, expected);
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_remove_tab_indent() -> Result<()> {
        let dir = tempdir()?;
        let tab_content = r#"{
    "alist": [
        2,
        34,
        234
    ],
    "blah": null,
    "foo": "bar"
}
"#;
        let file_path = create_test_file(&dir, "tab_indented.json", tab_content).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&output).contains("Fixed & autoformatted file"));

        let result = fs_err::tokio::read_to_string(&file_path).await?;
        assert_eq!(result, PRETTY_JSON);
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_ensure_ascii_uppercase_to_lowercase() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "non_ascii.json", NON_ASCII_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&output).contains("Fixed & autoformatted file"));

        let result = fs_err::tokio::read_to_string(&file_path).await?;
        let expected = r#"{
  "alist": [
    2,
    34,
    234
  ],
  "blah": null,
  "foo": "bar",
  "non_ascii": "\u4e2d\u6587\u306b\u307b\u3093\u3054\ud55c\uad6d\uc5b4"
}
"#;
        assert_eq!(result, expected);
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_ensure_ascii_already_lowercase() -> Result<()> {
        let dir = tempdir()?;
        let lowercase_content = NON_ASCII_JSON.to_lowercase();
        let file_path = create_test_file(&dir, "non_ascii.json", &lowercase_content).await?;

        let (code, _output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 0);
        let result = fs_err::tokio::read_to_string(&file_path).await?;
        let expected = r#"{
  "alist": [
    2,
    34,
    234
  ],
  "blah": null,
  "foo": "bar",
  "non_ascii": "\u4e2d\u6587\u306b\u307b\u3093\u3054\ud55c\uad6d\uc5b4"
}
"#;
        assert_eq!(result, expected);
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_no_ensure_ascii() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "non_ascii.json", NON_ASCII_JSON).await?;

        let (code, output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "2".to_string(),
                no_ensure_ascii: true,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        assert!(String::from_utf8_lossy(&output).contains("Fixed & autoformatted file"));

        let result = fs_err::tokio::read_to_string(&file_path).await?;
        let expected = r#"{
  "alist": [
    2,
    34,
    234
  ],
  "blah": null,
  "foo": "bar",
  "non_ascii": "中文にほんご한국어"
}
"#;
        assert_eq!(result, expected);
        dir.close()?;
        Ok(())
    }

    #[test]
    fn test_ensure_ascii_surrogate_pair_and_object_keys() -> Result<()> {
        let formatted = prettify_json(
            r#"{"emoji":"🐐","α":"beta"}"#,
            &Args {
                autofix: false,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )?;

        let expected = r#"{
  "emoji": "\ud83d\udc10",
  "\u03b1": "beta"
}
"#;
        assert_eq!(formatted, expected);
        Ok(())
    }

    #[tokio::test]
    async fn test_nested_objects() -> Result<()> {
        let dir = tempdir()?;
        let nested = r#"{"outer": {"inner": "value", "another": 123}, "top": true}"#;
        let file_path = create_test_file(&dir, "nested.json", nested).await?;

        let (code, _output) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        let result = fs_err::tokio::read_to_string(&file_path).await?;
        assert!(result.contains("\"another\": 123"));
        assert!(result.contains("\"inner\": \"value\""));
        dir.close()?;
        Ok(())
    }

    #[tokio::test]
    async fn test_array_preservation() -> Result<()> {
        let dir = tempdir()?;
        let array_json = r#"{"numbers": [5, 1, 9, 3], "sorted": false}"#;
        let file_path = create_test_file(&dir, "array.json", array_json).await?;

        let (code, _) = check_file(
            Path::new(""),
            &file_path,
            &Args {
                autofix: true,
                indent: "2".to_string(),
                no_ensure_ascii: false,
                no_sort_keys: false,
                top_keys: vec![],
            },
        )
        .await?;

        assert_eq!(code, 1);
        let result = fs_err::tokio::read_to_string(&file_path).await?;
        // Array order should be preserved
        assert!(
            result.contains("[5, 1, 9, 3]")
                || result.contains("[\n    5,\n    1,\n    9,\n    3\n  ]")
        );
        dir.close()?;
        Ok(())
    }
}
