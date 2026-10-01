use std::path::Path;

use anyhow::Result;
use xml::reader::ParserConfig;

use crate::hook::Hook;
use crate::hooks::HookOutput;
use crate::hooks::pre_commit_hooks::{FilenamesArgs, parse_hook_args, run_blocking_file_checks};

/// Runs the `check-xml` hook.
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

fn check_file(file_path: &Path, filename: &Path) -> Result<HookOutput> {
    let content = fs_err::read(file_path)?;

    // Parse the whole document once with xml-rs. This is stricter than the upstream Python
    // check-xml (xml.sax/Expat) in two cases:
    // - `<p:root/>` passes upstream because namespace processing is off; xml-rs requires
    //   `xmlns:p`.
    // - `<!DOCTYPE root SYSTEM "x.dtd"><root>&ext;</root>` passes upstream because `ext` may be
    //   declared by the unloaded DTD; xml-rs reports it as unknown.
    // Keeping these differences avoids rewriting or repeatedly parsing the input.
    let parser = ParserConfig::new()
        .allow_multiple_root_elements(false)
        .create_reader(content.as_slice());

    for event in parser {
        if let Err(error) = event {
            let error_message = format!("{}: Failed to xml parse ({error})\n", filename.display());
            return Ok(HookOutput::unchanged(1, error_message.into_bytes()));
        }
    }

    Ok(HookOutput::unchanged(0, Vec::new()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use tempfile::tempdir;

    fn create_test_file(dir: &tempfile::TempDir, name: &str, content: &[u8]) -> Result<PathBuf> {
        let file_path = dir.path().join(name);
        fs_err::write(&file_path, content)?;
        Ok(file_path)
    }

    #[test]
    fn test_valid_xml() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</element>
</root>"#;
        let file_path = create_test_file(&dir, "valid.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_valid_xml_with_leading_processing_instruction() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml-stylesheet href="style.xsl"?><root/>"#;
        let file_path = create_test_file(&dir, "processing_instruction.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_invalid_xml_unclosed_tag() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value
</root>"#;
        let file_path = create_test_file(&dir, "invalid.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        let output_str = String::from_utf8_lossy(&result.output);
        assert!(output_str.contains("Failed to xml parse"));
        Ok(())
    }

    #[test]
    fn test_invalid_xml_mismatched_tags() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element>value</different>
</root>"#;
        let file_path = create_test_file(&dir, "mismatched.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_invalid_xml_syntax_error() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element attribute="unclosed value>text</element>
</root>"#;
        let file_path = create_test_file(&dir, "syntax_error.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_invalid_xml_trailing_text() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "trailing.xml", b"<root/>junk")?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_empty_xml() -> Result<()> {
        let dir = tempdir()?;
        let content = b"";
        let file_path = create_test_file(&dir, "empty.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        let output_str = String::from_utf8_lossy(&result.output);
        assert!(output_str.contains("no root element found"));
        Ok(())
    }

    #[test]
    fn test_whitespace_only_xml() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "whitespace.xml", b" \n\t")?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_valid_xml_with_attributes() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root xmlns="http://example.com">
    <element id="1" type="test">value</element>
    <element id="2">another value</element>
</root>"#;
        let file_path = create_test_file(&dir, "attributes.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_invalid_xml_duplicate_attribute() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(
            &dir,
            "duplicate_attribute.xml",
            br#"<root key="1" key="2"/>"#,
        )?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_invalid_xml_element_name() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "invalid_name.xml", b"<1root/>")?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_valid_xml_with_cdata() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <element><![CDATA[Some <special> characters & symbols]]></element>
</root>"#;
        let file_path = create_test_file(&dir, "cdata.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_valid_xml_with_comments() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<root>
    <!-- This is a comment -->
    <element>value</element>
    <!-- Another comment -->
</root>"#;
        let file_path = create_test_file(&dir, "comments.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_xml_with_doctype() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE root SYSTEM "root.dtd">
<root>
    <element>value</element>
</root>"#;
        let file_path = create_test_file(&dir, "doctype.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_rejects_unresolved_external_dtd_entity() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<!DOCTYPE html SYSTEM "xhtml.dtd"><html>&nbsp;</html>"#;
        let file_path = create_test_file(&dir, "external_entity.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_invalid_xml_unknown_entity_without_external_dtd() -> Result<()> {
        let dir = tempdir()?;
        let file_path = create_test_file(&dir, "unknown_entity.xml", b"<root>&unknown;</root>")?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_xml_with_internal_entity() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<!DOCTYPE root [<!ENTITY value "ok">]>
<root>&value;</root>"#;
        let file_path = create_test_file(&dir, "internal_entity.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_valid_utf16_xml() -> Result<()> {
        let dir = tempdir()?;
        let mut content = vec![0xff, 0xfe];
        for code_unit in "<?xml version=\"1.0\" encoding=\"UTF-16\"?><root/>".encode_utf16() {
            content.extend_from_slice(&code_unit.to_le_bytes());
        }
        let file_path = create_test_file(&dir, "utf16.xml", &content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 0);
        assert_eq!(result.output, b"");
        Ok(())
    }

    #[test]
    fn test_invalid_xml_no_root() -> Result<()> {
        let dir = tempdir()?;
        let content = br#"<?xml version="1.0" encoding="UTF-8"?>
<element>value</element>
<another>value</another>"#;
        let file_path = create_test_file(&dir, "no_root.xml", content)?;
        let result = check_file(&file_path, &file_path)?;
        assert_eq!(result.exit_status, 1);
        assert_ne!(result.output, b"");
        Ok(())
    }
}
