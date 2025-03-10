use std::collections::HashMap;
use std::str::FromStr;

use anyhow::Result;
use url::Url;

use crate::hook::Hook;

mod fix_trailing_whitespace;

pub(crate) enum Implemented {
    TrailingWhitespace,
}

impl FromStr for Implemented {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trailing-whitespace" => Ok(Self::TrailingWhitespace),
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
        }
    }
}

// TODO: compare rev
pub(crate) fn is_pre_commit_hooks(url: &Url) -> bool {
    url.host_str() == Some("github.com") && url.path() == "/pre-commit/pre-commit-hooks"
}
