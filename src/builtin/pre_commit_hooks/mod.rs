use std::collections::HashMap;
use std::str::FromStr;

use anyhow::Result;
use url::Url;

use crate::hook::Hook;

mod check_added_large_files;
mod fix_end_of_file;
mod fix_trailing_whitespace;

pub(crate) enum Implemented {
    TrailingWhitespace,
    CheckAddedLargeFiles,
    EndOfFileFixer,
}

impl FromStr for Implemented {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trailing-whitespace" => Ok(Self::TrailingWhitespace),
            "check-added-large-files" => Ok(Self::CheckAddedLargeFiles),
            "end-of-file-fixer" => Ok(Self::EndOfFileFixer),
            _ => Err(()),
        }
    }
}

impl Implemented {
    pub(crate) async fn run(
        self,
        hook: &Hook,
        filenames: &[&String],
        env_vars: &HashMap<&'static str, String>,
    ) -> Result<(i32, Vec<u8>)> {
        match self {
            Self::TrailingWhitespace => {
                fix_trailing_whitespace::fix_trailing_whitespace(hook, filenames, env_vars).await
            }
            Self::CheckAddedLargeFiles => {
                check_added_large_files::check_added_large_files(hook, filenames, env_vars).await
            }
            Self::EndOfFileFixer => {
                fix_end_of_file::fix_end_of_file(hook, filenames, env_vars).await
            }
        }
    }
}

// TODO: compare rev
pub(crate) fn is_pre_commit_hooks(url: &Url) -> bool {
    url.host_str() == Some("github.com") && url.path() == "/pre-commit/pre-commit-hooks"
}
