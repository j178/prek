use std::path::Path;
use std::str::FromStr;

use anyhow::Result;
use tracing::debug;

use crate::hook::Hook;

mod check_added_large_files;
mod check_executables_have_shebangs;
mod check_json;
mod check_merge_conflict;
mod check_symlinks;
mod check_toml;
mod check_xml;
mod check_yaml;
mod detect_private_key;
mod fix_byte_order_marker;
mod fix_end_of_file;
mod fix_trailing_whitespace;
mod mixed_line_ending;
mod no_commit_to_branch;

pub(crate) use check_added_large_files::check_added_large_files;
pub(crate) use check_executables_have_shebangs::check_executables_have_shebangs;
pub(crate) use check_json::check_json;
pub(crate) use check_merge_conflict::check_merge_conflict;
pub(crate) use check_symlinks::check_symlinks;
pub(crate) use check_toml::check_toml;
pub(crate) use check_xml::check_xml;
pub(crate) use check_yaml::check_yaml;
pub(crate) use detect_private_key::detect_private_key;
pub(crate) use fix_byte_order_marker::fix_byte_order_marker;
pub(crate) use fix_end_of_file::fix_end_of_file;
pub(crate) use fix_trailing_whitespace::fix_trailing_whitespace;
pub(crate) use mixed_line_ending::mixed_line_ending;
pub(crate) use no_commit_to_branch::no_commit_to_branch;

/// Hooks from `https://github.com/pre-commit/pre-commit-hooks`.
pub(crate) enum PreCommitHooks {
    TrailingWhitespace,
    CheckAddedLargeFiles,
    EndOfFileFixer,
    FixByteOrderMarker,
    CheckJson,
    CheckSymlinks,
    CheckMergeConflict,
    CheckToml,
    CheckXml,
    CheckYaml,
    MixedLineEnding,
    DetectPrivateKey,
    NoCommitToBranch,
    CheckExecutablesHaveShebangs,
}

impl FromStr for PreCommitHooks {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "trailing-whitespace" => Ok(Self::TrailingWhitespace),
            "check-added-large-files" => Ok(Self::CheckAddedLargeFiles),
            "end-of-file-fixer" => Ok(Self::EndOfFileFixer),
            "fix-byte-order-marker" => Ok(Self::FixByteOrderMarker),
            "check-json" => Ok(Self::CheckJson),
            "check-merge-conflict" => Ok(Self::CheckMergeConflict),
            "check-toml" => Ok(Self::CheckToml),
            "check-symlinks" => Ok(Self::CheckSymlinks),
            "check-xml" => Ok(Self::CheckXml),
            "check-yaml" => Ok(Self::CheckYaml),
            "mixed-line-ending" => Ok(Self::MixedLineEnding),
            "detect-private-key" => Ok(Self::DetectPrivateKey),
            "no-commit-to-branch" => Ok(Self::NoCommitToBranch),
            "check-executables-have-shebangs" => Ok(Self::CheckExecutablesHaveShebangs),
            _ => Err(()),
        }
    }
}

impl PreCommitHooks {
    pub(crate) fn check_supported(&self, hook: &Hook) -> bool {
        match self {
            // `check-yaml` does not support `--unsafe` flag yet.
            Self::CheckYaml => !hook.args.iter().any(|s| s.starts_with("--unsafe")),
            _ => true,
        }
    }

    pub(crate) async fn run(self, hook: &Hook, filenames: &[&Path]) -> Result<(i32, Vec<u8>)> {
        debug!("Running hook `{}` in fast path", hook.id);
        match self {
            Self::TrailingWhitespace => fix_trailing_whitespace(hook, filenames).await,
            Self::CheckAddedLargeFiles => check_added_large_files(hook, filenames).await,
            Self::EndOfFileFixer => fix_end_of_file(hook, filenames).await,
            Self::FixByteOrderMarker => fix_byte_order_marker(hook, filenames).await,
            Self::CheckJson => check_json(hook, filenames).await,
            Self::CheckSymlinks => check_symlinks(hook, filenames).await,
            Self::CheckMergeConflict => check_merge_conflict(hook, filenames).await,
            Self::CheckToml => check_toml(hook, filenames).await,
            Self::CheckYaml => check_yaml(hook, filenames).await,
            Self::CheckXml => check_xml(hook, filenames).await,
            Self::MixedLineEnding => mixed_line_ending(hook, filenames).await,
            Self::DetectPrivateKey => detect_private_key(hook, filenames).await,
            Self::NoCommitToBranch => no_commit_to_branch(hook).await,
            Self::CheckExecutablesHaveShebangs => {
                check_executables_have_shebangs(hook, filenames).await
            }
        }
    }
}

// TODO: compare rev
pub(crate) fn is_pre_commit_hooks(url: &str) -> bool {
    url == "https://github.com/pre-commit/pre-commit-hooks"
}
