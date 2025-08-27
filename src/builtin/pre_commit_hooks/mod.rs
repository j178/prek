use std::str::FromStr;

use anyhow::Result;

use crate::hook::Hook;

mod check_added_large_files;
mod check_json;
mod check_toml;
mod check_yaml;
mod fix_end_of_file;
mod fix_trailing_whitespace;
mod mixed_line_ending;

pub(crate) enum Implemented {
    TrailingWhitespace,
    CheckAddedLargeFiles,
    EndOfFileFixer,
    CheckJson,
    CheckToml,
    CheckYaml,
    MixedLineEnding,
}

impl FromStr for Implemented {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trailing-whitespace" => Ok(Self::TrailingWhitespace),
            "check-added-large-files" => Ok(Self::CheckAddedLargeFiles),
            "end-of-file-fixer" => Ok(Self::EndOfFileFixer),
            "check-json" => Ok(Self::CheckJson),
            "check-toml" => Ok(Self::CheckToml),
            "check-yaml" => Ok(Self::CheckYaml),
            "mixed-line-ending" => Ok(Self::MixedLineEnding),
            _ => Err(()),
        }
    }
}

impl Implemented {
    pub(crate) fn check_supported(&self, hook: &Hook) -> bool {
        match self {
            // `check-yaml` does not support `--unsafe` or `--allow-multiple-documents` flags yet.
            Self::CheckYaml => !hook
                .args
                .iter()
                .any(|s| s.starts_with("--unsafe") || s.starts_with("--allow-multiple-documents")),
            _ => true,
        }
    }

    pub(crate) async fn run(self, hook: &Hook, filenames: &[&String]) -> Result<(i32, Vec<u8>)> {
        match self {
            Self::TrailingWhitespace => {
                fix_trailing_whitespace::fix_trailing_whitespace(hook, filenames).await
            }
            Self::CheckAddedLargeFiles => {
                check_added_large_files::check_added_large_files(hook, filenames).await
            }
            Self::EndOfFileFixer => fix_end_of_file::fix_end_of_file(hook, filenames).await,
            Self::CheckJson => check_json::check_json(hook, filenames).await,
            Self::CheckToml => check_toml::check_toml(hook, filenames).await,
            Self::CheckYaml => check_yaml::check_yaml(hook, filenames).await,
            Self::MixedLineEnding => mixed_line_ending::mixed_line_ending(hook, filenames).await,
        }
    }
}

// TODO: compare rev
pub(crate) fn is_pre_commit_hooks(url: &str) -> bool {
    url == "https://github.com/pre-commit/pre-commit-hooks"
}
