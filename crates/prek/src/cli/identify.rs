use std::fmt::Write;
use std::path::PathBuf;

use anyhow::Context;
use owo_colors::OwoColorize;
use serde::Serialize;

use crate::cli::{ExitStatus, IdentifyOutputFormat};
use crate::identify::tags_from_path;
use crate::printer::Printer;

#[derive(Serialize)]
struct SerializableIdentify {
    path: String,
    tags: Vec<String>,
}

pub(crate) fn identify(
    paths: &[PathBuf],
    output_format: IdentifyOutputFormat,
    printer: Printer,
) -> anyhow::Result<ExitStatus> {
    for path in paths {
        let tags = tags_from_path(path)
            .with_context(|| format!("Failed to identify file: {}", path.display()))?;

        let tags_vec: Vec<String> = tags.iter().map(std::string::ToString::to_string).collect();

        match output_format {
            IdentifyOutputFormat::Text => {
                writeln!(printer.stdout(), "{}", path.display().bold())?;
                writeln!(
                    printer.stdout(),
                    "  {} {}",
                    "Tags:".bold().cyan(),
                    tags_vec.join(", ")
                )?;
                writeln!(printer.stdout())?;
            }
            IdentifyOutputFormat::Json => {
                let serializable = SerializableIdentify {
                    path: path.display().to_string(),
                    tags: tags_vec,
                };
                let json_output = serde_json::to_string_pretty(&serializable)?;
                writeln!(printer.stdout(), "{json_output}")?;
            }
        }
    }

    Ok(ExitStatus::Success)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serializable_identify_serialize() {
        let obj = SerializableIdentify {
            path: "/test/path.py".to_string(),
            tags: vec!["file".to_string(), "python".to_string(), "text".to_string()],
        };

        let json = serde_json::to_string_pretty(&obj).unwrap();
        assert!(json.contains("/test/path.py"));
        assert!(json.contains("python"));
        assert!(json.contains("file"));
    }

    #[test]
    fn test_serializable_identify_serialize_empty_tags() {
        let obj = SerializableIdentify {
            path: "/test/path".to_string(),
            tags: vec![],
        };

        let json = serde_json::to_string_pretty(&obj).unwrap();
        assert!(json.contains("/test/path"));
        assert!(json.contains("\"tags\": []"));
    }
}
