use std::str::FromStr;

use anyhow::Result;
use url::Url;

use crate::hook::Hook;

mod check_added_large_files;
mod check_json;
mod fix_end_of_file;
mod fix_trailing_whitespace;
mod mixed_line_ending;

pub(crate) enum Implemented {
    TrailingWhitespace,
    CheckAddedLargeFiles,
    EndOfFileFixer,
    CheckJson,
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
            "mixed-line-ending" => Ok(Self::MixedLineEnding),
            _ => Err(()),
        }
    }
}

impl Implemented {
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
            Self::MixedLineEnding => mixed_line_ending::mixed_line_ending(hook, filenames).await,
        }
    }
}

// TODO: compare rev
pub(crate) fn is_pre_commit_hooks(url: &Url) -> bool {
    url.host_str() == Some("github.com") && url.path() == "/pre-commit/pre-commit-hooks"
}
