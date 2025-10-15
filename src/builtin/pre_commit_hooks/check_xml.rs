use std::path::Path;

use anyhow::Result;
use futures::StreamExt;

use crate::hook::Hook;
use crate::run::CONCURRENCY;

pub(crate) async fn check_xml(hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
    let mut tasks = futures::stream::iter(filenames)
        .map(async |filename| check_file(hook.project().relative_path(), filename).await)
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

async fn check_file(file_base: &Path, filename: &Path) -> Result<(i32, Vec<u8>)> {
    let content = fs_err::tokio::read(file_base.join(filename)).await?;

    // Empty XML is invalid - should have at least one element
    if content.is_empty() {
        let error_message = format!(
            "{}: Failed to xml parse (no element found)\n",
            filename.display()
        );
        return Ok((1, error_message.into_bytes()));
    }

    let mut reader = quick_xml::Reader::from_reader(&content[..]);
    reader.config_mut().check_end_names = true;
    reader.config_mut().expand_empty_elements = true;

    let mut buf = Vec::new();
    let mut root_count = 0;
    let mut depth = 0;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(quick_xml::events::Event::Eof) => break,
            Ok(quick_xml::events::Event::Start(_)) => {
                if depth == 0 {
                    root_count += 1;
                    if root_count > 1 {
                        let error_message = format!(
                            "{}: Failed to xml parse (junk after document element)\n",
                            filename.display()
                        );
                        return Ok((1, error_message.into_bytes()));
                    }
                }
                depth += 1;
            }
            Ok(quick_xml::events::Event::End(_)) => {
                depth -= 1;
            }
            Err(e) => {
                let error_message = format!("{}: Failed to xml parse ({e})\n", filename.display());
                return Ok((1, error_message.into_bytes()));
            }
            Ok(_) => {}
        }
        buf.clear();
    }

    Ok((0, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
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
    async fn test_valid_xml() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
</root>"#;
        let file_path = create_test_file(&dir, "valid.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_xml_unclosed_tag() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value
</root>"#;
        let file_path = create_test_file(&dir, "invalid.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("Failed to xml parse"));
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_xml_mismatched_tags() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</different>
</root>"#;
        let file_path = create_test_file(&dir, "mismatched.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_xml_syntax_error() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element attribute="unclosed value>text</element>
</root>"#;
        let file_path = create_test_file(&dir, "syntax_error.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_empty_xml() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1); // Changed from 0 to 1
        assert!(!output.is_empty()); // Changed from is_empty() to !is_empty()
        let output_str = String::from_utf8_lossy(&output);
        assert!(output_str.contains("no element found"));
        Ok(())
    }

    #[tokio::test]
    async fn test_valid_xml_with_attributes() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="http://example.com">
    <element id="1" type="test">value</element>
    <element id="2">another value</element>
</root>"#;
        let file_path = create_test_file(&dir, "attributes.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_valid_xml_with_cdata() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element><![CDATA[Some <special> characters & symbols]]></element>
</root>"#;
        let file_path = create_test_file(&dir, "cdata.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_valid_xml_with_comments() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <!-- This is a comment -->
    <element>value</element>
    <!-- Another comment -->
</root>"#;
        let file_path = create_test_file(&dir, "comments.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_xml_with_doctype() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE root SYSTEM "root.dtd">
<root>
    <element>value</element>
</root>"#;
        let file_path = create_test_file(&dir, "doctype.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 0);
        assert!(output.is_empty());
        Ok(())
    }

    #[tokio::test]
    async fn test_invalid_xml_no_root() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<element>value</element>
<another>value</another>"#;
        let file_path = create_test_file(&dir, "no_root.xml", content).await?;
        let (code, output) = check_file(Path::new(""), &file_path).await?;
        assert_eq!(code, 1);
        assert!(!output.is_empty());
        Ok(())
    }
}
